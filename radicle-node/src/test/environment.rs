use std::io::BufRead as _;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs, io, iter, net, process, thread, time,
    time::Duration,
};

use crossbeam_channel as chan;

use radicle::cob;
use radicle::cob::issue;
use radicle::crypto::ssh::{keystore::MemorySigner, Keystore};
use radicle::crypto::test::signer::MockSigner;
use radicle::crypto::{KeyPair, Seed, Signer};
use radicle::git;
use radicle::git::refname;
use radicle::identity::Id;
use radicle::node::address::Book;
use radicle::node::routing;
use radicle::node::routing::Store;
use radicle::node::tracking::store as tracking;
use radicle::node::{Alias, ADDRESS_DB_FILE, ROUTING_DB_FILE, TRACKING_DB_FILE};
use radicle::node::{ConnectOptions, Handle as _};
use radicle::profile;
use radicle::profile::Home;
use radicle::profile::Profile;
use radicle::rad;
use radicle::storage::{ReadRepository, ReadStorage as _, SignRepository as _};
use radicle::test::fixtures;
use radicle::Storage;

use crate::node::NodeId;
use crate::service::Event;
use crate::storage::git::transport;
use crate::{runtime, runtime::Handle, service, Runtime};

pub use service::Config;

/// Test environment.
pub struct Environment {
    tempdir: tempfile::TempDir,
    users: usize,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            tempdir: tempfile::tempdir().unwrap(),
            users: 0,
        }
    }
}

impl Environment {
    /// Create a new test environment.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the temp directory path.
    pub fn tmp(&self) -> PathBuf {
        self.tempdir.path().join("misc")
    }

    /// Get the scale or "test size". This is used to scale tests with more data. Defaults to `1`.
    pub fn scale(&self) -> usize {
        env::var("RAD_TEST_SCALE")
            .map(|s| {
                s.parse()
                    .expect("repository: invalid value for `RAD_TEST_SCALE`")
            })
            .unwrap_or(1)
    }

    /// Create a new node in this environment. This should be used when a running node
    /// is required. Use [`Environment::profile`] otherwise.
    pub fn node(&mut self, config: Config) -> Node<MemorySigner> {
        let profile = self.profile(&config.alias);
        let signer = MemorySigner::load(&profile.keystore, None).unwrap();

        let tracking_db = profile.home.node().join(TRACKING_DB_FILE);
        let tracking = tracking::Config::open(tracking_db).unwrap();

        let routing_db = profile.home.node().join(ROUTING_DB_FILE);
        let routing = routing::Table::open(routing_db).unwrap();

        let addresses_db = profile.home.node().join(ADDRESS_DB_FILE);
        let addresses = Book::open(addresses_db).unwrap();

        Node {
            id: *profile.id(),
            home: profile.home,
            config,
            signer,
            addresses,
            routing,
            tracking,
            storage: profile.storage,
        }
    }

    /// Create a new profile in this environment.
    /// This should be used when a running node is not required.
    pub fn profile(&mut self, alias: &str) -> Profile {
        let home = Home::new(self.tmp().join("home").join(alias).join(".radicle")).unwrap();
        let storage = Storage::open(home.storage()).unwrap();
        let keystore = Keystore::new(&home.keys());
        let keypair = KeyPair::from_seed(Seed::from([!(self.users as u8); 32]));
        let tracking_db = home.node().join(TRACKING_DB_FILE);
        let alias = Alias::from_str(alias).unwrap();
        let config = profile::Config::init(alias, &home.config()).unwrap();

        tracking::Config::open(tracking_db).unwrap();
        let addresses_db = home.node().join(ADDRESS_DB_FILE);
        Book::open(addresses_db).unwrap();

        transport::local::register(storage.clone());
        keystore.store(keypair.clone(), "radicle", None).unwrap();

        // Ensures that each user has a unique but deterministic public key.
        self.users += 1;

        Profile {
            home,
            storage,
            keystore,
            public_key: keypair.pk.into(),
            config,
        }
    }
}

/// A node that can be run.
pub struct Node<G> {
    pub id: NodeId,
    pub home: Home,
    pub signer: G,
    pub storage: Storage,
    pub config: Config,
    pub addresses: Book,
    pub routing: routing::Table,
    pub tracking: tracking::Config<tracking::Write>,
}

/// Handle to a running node.
pub struct NodeHandle<G: Signer + cyphernet::Ecdh + 'static> {
    pub id: NodeId,
    pub storage: Storage,
    pub signer: G,
    pub home: Home,
    pub addr: net::SocketAddr,
    pub thread: ManuallyDrop<thread::JoinHandle<Result<(), runtime::Error>>>,
    pub handle: ManuallyDrop<Handle>,
}

impl<G: Signer + cyphernet::Ecdh + 'static> Drop for NodeHandle<G> {
    fn drop(&mut self) {
        log::debug!(target: "test", "Node {} shutting down..", self.id);

        unsafe { ManuallyDrop::take(&mut self.handle) }
            .shutdown()
            .unwrap();
        unsafe { ManuallyDrop::take(&mut self.thread) }
            .join()
            .unwrap()
            .unwrap();
    }
}

impl<G: Signer + cyphernet::Ecdh> NodeHandle<G> {
    /// Connect this node to another node, and wait for the connection to be established both ways.
    pub fn connect(&mut self, remote: &NodeHandle<G>) -> &mut Self {
        let local_events = self.handle.events();
        let remote_events = remote.handle.events();

        self.handle
            .connect(remote.id, remote.addr.into(), ConnectOptions::default())
            .ok();

        local_events
            .iter()
            .find(|e| {
                matches!(
                    e, Event::PeerConnected { nid } if nid == &remote.id
                )
            })
            .unwrap();
        remote_events
            .iter()
            .find(|e| {
                matches!(
                    e, Event::PeerConnected { nid } if nid == &self.id
                )
            })
            .unwrap();

        self
    }

    /// Get routing table entries.
    pub fn routing(&self) -> impl Iterator<Item = (Id, NodeId)> {
        radicle::node::routing::Table::reader(self.home.node().join(radicle::node::ROUTING_DB_FILE))
            .unwrap()
            .entries()
            .unwrap()
    }

    /// Wait until this node's routing table matches the remotes.
    pub fn converge<'a>(
        &'a self,
        remotes: impl IntoIterator<Item = &'a NodeHandle<G>>,
    ) -> BTreeSet<(Id, NodeId)> {
        converge(iter::once(self).chain(remotes.into_iter()))
    }

    /// Wait until this node's routing table contains the given routes.
    #[track_caller]
    pub fn routes_to(&self, routes: &[(Id, NodeId)]) {
        log::debug!(target: "test", "Waiting for {} to route to {:?}", self.id, routes);
        let events = self.handle.events();

        loop {
            let mut remaining: BTreeSet<_> = routes.iter().collect();

            for (rid, nid) in self.routing() {
                if !remaining.remove(&(rid, nid)) {
                    panic!(
                        "Node::routes_to: unexpected route for {}: ({rid}, {nid})",
                        self.id
                    );
                }
            }
            if remaining.is_empty() {
                break;
            }
            events
                .wait(
                    |e| matches!(e, Event::SeedDiscovered { .. }).then_some(()),
                    time::Duration::from_secs(6),
                )
                .unwrap();
        }
    }

    /// Wait until this node has the inventory of another node.
    #[track_caller]
    pub fn has_inventory_of(&self, rid: &Id, nid: &NodeId) {
        log::debug!(target: "test", "Waiting for {} to have {rid}/{nid}", self.id);
        let events = self.handle.events();

        loop {
            if let Ok(repo) = self.storage.repository(*rid) {
                if repo.identity_of(nid).is_ok() && repo.remote(nid).is_ok() {
                    break;
                }
            }
            events
                .wait(
                    |e| matches!(e, Event::RefsFetched { .. }).then_some(()),
                    time::Duration::from_secs(6),
                )
                .unwrap();
        }
    }

    /// Run a `rad` CLI command.
    pub fn rad<P: AsRef<Path>>(&self, cmd: &str, args: &[&str], cwd: P) -> io::Result<()> {
        let cwd = cwd.as_ref();
        log::debug!(target: "test", "Running `rad {cmd} {args:?}` in {}..", cwd.display());

        fs::create_dir_all(cwd)?;

        let result = process::Command::new(snapbox::cmd::cargo_bin("rad"))
            .env_clear()
            .envs(env::vars().filter(|(k, _)| k == "PATH"))
            .env("GIT_AUTHOR_DATE", "1671125284")
            .env("GIT_AUTHOR_EMAIL", "radicle@localhost")
            .env("GIT_AUTHOR_NAME", "radicle")
            .env("GIT_COMMITTER_DATE", "1671125284")
            .env("GIT_COMMITTER_EMAIL", "radicle@localhost")
            .env("GIT_COMMITTER_NAME", "radicle")
            .env("RAD_HOME", self.home.path().to_string_lossy().to_string())
            .env("RAD_PASSPHRASE", "radicle")
            .env("TZ", "UTC")
            .env("LANG", "C")
            .envs(git::env::GIT_DEFAULT_CONFIG)
            .current_dir(cwd)
            .arg(cmd)
            .args(args)
            .output()?;

        for line in io::BufReader::new(io::Cursor::new(&result.stdout))
            .lines()
            .flatten()
        {
            log::debug!(target: "test", "rad {cmd}: {line}");
        }

        log::debug!(
            target: "test",
            "Ran command `rad {cmd}` (status={})", result.status.code().unwrap()
        );

        if !result.status.success() {
            return Err(io::ErrorKind::Other.into());
        }
        Ok(())
    }

    /// Create an [`issue::Issue`] in the `NodeHandle`'s storage.
    pub fn issue(&self, rid: Id, title: &str, desc: &str) -> cob::ObjectId {
        let repo = self.storage.repository(rid).unwrap();
        let mut issues = issue::Issues::open(&repo).unwrap();
        *issues
            .create(title, desc, &[], &[], &self.signer)
            .unwrap()
            .id()
    }

    /// Helper to print out references for debugging purposes.
    pub fn inspect_refs(&self, rid: Id) {
        let repo = self.storage.repository(rid).unwrap();
        for r in repo.references().unwrap() {
            let r = r.unwrap();
            match r.namespace {
                Some(ns) => println!("refs/namespaces/{ns}/{}->{}", r.name, r.oid),
                None => println!("{}->{}", r.name, r.oid),
            }
        }
    }
}

impl Node<MockSigner> {
    /// Create a new node.
    pub fn init(base: &Path, config: Config) -> Self {
        let home = base.join(
            iter::repeat_with(fastrand::alphanumeric)
                .take(8)
                .collect::<String>(),
        );
        let home = Home::new(home).unwrap();
        let signer = MockSigner::default();
        let storage = Storage::open(home.storage()).unwrap();
        let addresses = Book::memory().unwrap();
        let tracking = tracking::Config::<tracking::Write>::memory().unwrap();
        let routing = routing::Table::memory().unwrap();

        log::debug!(target: "test", "Node::init {}: {}", config.alias, signer.public_key());
        Self {
            id: *signer.public_key(),
            home,
            signer,
            storage,
            config,
            addresses,
            tracking,
            routing,
        }
    }
}

impl<G: cyphernet::Ecdh<Pk = NodeId> + Signer + Clone> Node<G> {
    /// Spawn a node in its own thread.
    pub fn spawn(self) -> NodeHandle<G> {
        let listen = vec![([0, 0, 0, 0], 0).into()];
        let proxy = net::SocketAddr::new(net::Ipv4Addr::LOCALHOST.into(), 9050);
        let daemon: net::SocketAddr = {
            // Find free port for git-daemon to bind to.
            // This is a somewhat racy solution, though it works much better than assigning a random
            // port.
            let sock = net::TcpListener::bind("0.0.0.0:0").unwrap();
            ([0, 0, 0, 0], sock.local_addr().unwrap().port()).into()
        };
        let (_, signals) = chan::bounded(1);
        let rt = Runtime::init(
            self.home.clone(),
            self.config,
            listen,
            proxy,
            daemon,
            signals,
            self.signer.clone(),
        )
        .unwrap();
        let addr = *rt.local_addrs.first().unwrap();
        let id = *self.signer.public_key();
        let handle = ManuallyDrop::new(rt.handle.clone());
        let thread = ManuallyDrop::new(runtime::thread::spawn(&id, "runtime", move || rt.run()));

        NodeHandle {
            id,
            storage: self.storage,
            signer: self.signer,
            home: self.home,
            addr,
            handle,
            thread,
        }
    }

    /// Populate a storage instance with a project from the given repository.
    pub fn project_from(
        &mut self,
        name: &str,
        description: &str,
        repo: &git::raw::Repository,
    ) -> Id {
        transport::local::register(self.storage.clone());

        let id = rad::init(
            repo,
            name,
            description,
            refname!("master"),
            &self.signer,
            &self.storage,
        )
        .map(|(id, _, _)| id)
        .unwrap();

        log::debug!(
            target: "test",
            "Initialized project {id} for node {}", self.signer.public_key()
        );

        // Push local branches to storage.
        let mut refs = Vec::<(git::Qualified, git::Qualified)>::new();
        for branch in repo.branches(Some(git::raw::BranchType::Local)).unwrap() {
            let (branch, _) = branch.unwrap();
            let name = git::RefString::try_from(branch.name().unwrap().unwrap()).unwrap();

            refs.push((
                git::lit::refs_heads(&name).into(),
                git::lit::refs_heads(&name).into(),
            ));
        }
        git::push(repo, "rad", refs.iter().map(|(a, b)| (a, b))).unwrap();

        self.storage
            .repository(id)
            .unwrap()
            .sign_refs(&self.signer)
            .unwrap();

        id
    }

    /// Populate a storage instance with a project.
    pub fn project(&mut self, name: &str, description: &str) -> Id {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, _) = fixtures::repository(tmp.path());

        self.project_from(name, description, &repo)
    }
}

/// Checks whether the nodes have converged in their routing tables.
#[track_caller]
pub fn converge<'a, G: Signer + cyphernet::Ecdh + 'static>(
    nodes: impl IntoIterator<Item = &'a NodeHandle<G>>,
) -> BTreeSet<(Id, NodeId)> {
    let nodes = nodes.into_iter().collect::<Vec<_>>();

    let mut all_routes = BTreeSet::<(Id, NodeId)>::new();
    let mut remaining = BTreeMap::from_iter(nodes.iter().map(|node| (node.id, node)));

    // First build the set of all routes.
    for node in &nodes {
        // Routes from the routing table.
        for (rid, seed_id) in node.routing() {
            all_routes.insert((rid, seed_id));
        }
        // Routes from the local inventory.
        for rid in node.storage.inventory().unwrap() {
            all_routes.insert((rid, node.id));
        }
    }

    // Then, while there are nodes remaining to converge, check each node to see if
    // its routing table has all routes. If so, remove it from the remaining nodes.
    while !remaining.is_empty() {
        remaining.retain(|_, node| {
            let routing = node.routing();
            let routes = BTreeSet::from_iter(routing);

            if routes == all_routes {
                log::debug!(target: "test", "Node {} has converged", node.id);
                return false;
            } else {
                log::debug!(target: "test", "Node {} has {:?}", node.id, routes);
            }
            true
        });
        thread::sleep(Duration::from_millis(100));
    }
    all_routes
}
