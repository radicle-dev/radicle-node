pub mod handle;
pub mod thread;

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{fs, io, net, time};

use crossbeam_channel as chan;
use cyphernet::Ecdh;
use netservices::resource::NetAccept;
use radicle_fetch::FetchLimit;
use reactor::poller::popol;
use reactor::Reactor;
use thiserror::Error;

use radicle::git;
use radicle::node;
use radicle::node::address;
use radicle::node::address::Store as _;
use radicle::node::Handle as _;
use radicle::node::{ADDRESS_DB_FILE, NODE_ANNOUNCEMENT_FILE, ROUTING_DB_FILE, TRACKING_DB_FILE};
use radicle::profile::Home;
use radicle::Storage;

use crate::control;
use crate::crypto::Signer;
use crate::node::{routing, NodeId};
use crate::service::message::NodeAnnouncement;
use crate::service::{tracking, Event};
use crate::wire::Wire;
use crate::wire::{self, Decode};
use crate::worker;
use crate::{service, LocalTime};

pub use handle::Error as HandleError;
pub use handle::Handle;

/// A client error.
#[derive(Error, Debug)]
pub enum Error {
    /// A routing database error.
    #[error("routing database error: {0}")]
    Routing(#[from] routing::Error),
    /// An address database error.
    #[error("address database error: {0}")]
    Addresses(#[from] address::Error),
    /// A tracking database error.
    #[error("tracking database error: {0}")]
    Tracking(#[from] tracking::Error),
    /// An I/O error.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    /// A control socket error.
    #[error("control socket error: {0}")]
    Control(#[from] control::Error),
    /// Another node is already running.
    #[error(
        "another node appears to be running; \
        if this isn't the case, delete the socket file at '{0}' \
        and restart the node"
    )]
    AlreadyRunning(PathBuf),
    /// A git version error.
    #[error("git version error: {0}")]
    GitVersion(#[from] git::VersionError),
}

/// Publishes events to subscribers.
#[derive(Debug, Clone)]
pub struct Emitter<T> {
    subscribers: Arc<Mutex<Vec<chan::Sender<T>>>>,
}

impl<T> Default for Emitter<T> {
    fn default() -> Emitter<T> {
        Emitter {
            subscribers: Default::default(),
        }
    }
}

impl<T: Clone> Emitter<T> {
    /// Emit event to subscribers and drop those who can't receive it.
    pub(crate) fn emit(&self, event: T) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|s| s.try_send(event.clone()).is_ok());
    }

    /// Subscribe to events stream.
    pub fn subscribe(&self) -> chan::Receiver<T> {
        let (sender, receiver) = chan::unbounded();
        let mut subs = self.subscribers.lock().unwrap();
        subs.push(sender);

        receiver
    }
}

/// Holds join handles to the client threads, as well as a client handle.
pub struct Runtime {
    pub id: NodeId,
    pub home: Home,
    pub control: UnixListener,
    pub handle: Handle,
    pub storage: Storage,
    pub reactor: Reactor<wire::Control, popol::Poller>,
    pub pool: worker::Pool,
    pub local_addrs: Vec<net::SocketAddr>,
    pub signals: chan::Receiver<()>,
}

impl Runtime {
    /// Initialize the runtime.
    ///
    /// This function spawns threads.
    pub fn init<G: Signer + Ecdh + 'static>(
        home: Home,
        config: service::Config,
        listen: Vec<net::SocketAddr>,
        proxy: net::SocketAddr,
        signals: chan::Receiver<()>,
        signer: G,
    ) -> Result<Runtime, Error>
    where
        G: Ecdh<Pk = NodeId> + Clone,
    {
        let id = *signer.public_key();
        let alias = config.alias.clone();
        let node_dir = home.node();
        let network = config.network;
        let rng = fastrand::Rng::new();
        let clock = LocalTime::now();
        let storage = Storage::open(
            home.storage(),
            git::UserInfo {
                alias: alias.clone(),
                key: id,
            },
        )?;
        let address_db = node_dir.join(ADDRESS_DB_FILE);
        let routing_db = node_dir.join(ROUTING_DB_FILE);
        let tracking_db = node_dir.join(TRACKING_DB_FILE);
        let scope = config.scope;
        let policy = config.policy;

        log::info!(target: "node", "Opening address book {}..", address_db.display());
        let mut addresses = address::Book::open(address_db)?;

        log::info!(target: "node", "Opening routing table {}..", routing_db.display());
        let routing = routing::Table::open(routing_db)?;

        log::info!(target: "node", "Opening tracking policy table {}..", tracking_db.display());
        let tracking = tracking::Store::open(tracking_db.clone())?;
        let tracking = tracking::Config::new(policy, scope, tracking);

        log::info!(target: "node", "Default tracking policy set to '{}'", &policy);
        log::info!(target: "node", "Initializing service ({:?})..", network);

        let announcement = if let Some(ann) = fs::read(&node_dir.join(NODE_ANNOUNCEMENT_FILE))
            .ok()
            .and_then(|ann| NodeAnnouncement::decode(&mut ann.as_slice()).ok())
            .and_then(|ann| {
                if config.features() == ann.features
                    && config.alias == ann.alias
                    && config.external_addresses == ann.addresses.as_ref()
                {
                    Some(ann)
                } else {
                    None
                }
            }) {
            log::info!(
                target: "node",
                "Loaded existing node announcement from file (timestamp={}, work={})",
                ann.timestamp,
                ann.work(),
            );
            ann
        } else {
            service::gossip::node(&config, clock.as_secs())
                .solve(Default::default())
                .expect("Runtime::init: unable to solve proof-of-work puzzle")
        };

        if config.connect.is_empty() && addresses.is_empty()? {
            log::info!(target: "node", "Address book is empty. Adding bootstrap nodes..");

            for (alias, addr) in config.network.bootstrap() {
                let (id, addr) = addr.into();

                addresses.insert(
                    &id,
                    radicle::node::Features::SEED,
                    alias,
                    0,
                    clock.as_secs(),
                    [node::KnownAddress::new(addr, address::Source::Bootstrap)],
                )?;
            }
            log::info!(target: "node", "{} nodes added to address book", addresses.len()?);
        }

        let emitter: Emitter<Event> = Default::default();
        let service = service::Service::new(
            config,
            clock,
            routing,
            storage.clone(),
            addresses,
            tracking,
            signer.clone(),
            rng,
            announcement,
            emitter.clone(),
        );

        let (worker_send, worker_recv) = chan::unbounded::<worker::Task>();
        let mut wire = Wire::new(service, worker_send, signer.clone(), proxy, clock);
        let mut local_addrs = Vec::new();

        for addr in listen {
            let listener = NetAccept::bind(&addr)?;
            let local_addr = listener.local_addr();

            local_addrs.push(local_addr);
            wire.listen(listener);

            log::info!(target: "node", "Listening on {local_addr}..");
        }
        let reactor = Reactor::named(wire, popol::Poller::new(), thread::name(&id, "service"))?;
        let handle = Handle::new(home.clone(), reactor.controller(), emitter);
        let atomic = git::version()? >= git::VERSION_REQUIRED;

        if !atomic {
            log::warn!(
                target: "node",
                "Disabling atomic fetches; git version >= {} required", git::VERSION_REQUIRED
            );
        }

        let nid = *signer.public_key();
        let fetch = worker::FetchConfig {
            policy,
            scope,
            tracking_db,
            limit: FetchLimit::default(),
            info: git::UserInfo { alias, key: nid },
            local: nid,
        };
        let pool = worker::Pool::with(
            worker_recv,
            nid,
            handle.clone(),
            worker::Config {
                capacity: 8,
                timeout: time::Duration::from_secs(9),
                storage: storage.clone(),
                fetch,
            },
        );
        let control = match UnixListener::bind(home.socket()) {
            Ok(sock) => sock,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                return Err(Error::AlreadyRunning(home.socket()));
            }
            Err(err) => {
                return Err(err.into());
            }
        };

        Ok(Runtime {
            id,
            home,
            control,
            storage,
            reactor,
            handle,
            pool,
            signals,
            local_addrs,
        })
    }

    pub fn run(self) -> Result<(), Error> {
        let home = self.home;

        log::info!(target: "node", "Running node {} in {}..", self.id, home.path().display());
        log::info!(target: "node", "Binding control socket {}..", home.socket().display());

        thread::spawn(&self.id, "control", {
            let handle = self.handle.clone();
            || control::listen(self.control, handle)
        });
        let _signals = thread::spawn(&self.id, "signals", move || {
            if let Ok(()) = self.signals.recv() {
                log::info!(target: "node", "Termination signal received; shutting down..");
                self.handle.shutdown().ok();
            }
        });

        self.pool.run().unwrap();
        self.reactor.join().unwrap();

        // Nb. We don't join the control thread here, as we have no way of notifying it that the
        // node is shutting down.

        // Remove control socket file, but don't freak out if it's not there anymore.
        fs::remove_file(home.socket()).ok();

        log::debug!(target: "node", "Node shutdown completed for {}", self.id);

        Ok(())
    }
}
