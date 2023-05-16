pub(crate) mod fetch;
pub(crate) mod ls_refs;

use std::collections::BTreeSet;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gix_features::progress::prodash::progress;
use gix_hash::ObjectId;
use gix_object::bstr::BString;
use gix_protocol::handshake;
use gix_protocol::FetchConnection;
use gix_transport::client;
use gix_transport::client::TransportWithoutIO as _;
use gix_transport::Protocol;
use gix_transport::Service;
use nonempty::NonEmpty;
use thiserror::Error;
use versions::Version;

use crate::gix::odb;
use crate::gix::refdb;
use crate::gix::Refdb;
use crate::refs::ReceivedRef;

/// Open a reader and writer stream to pass to the ls-refs and fetch
/// processes for communicating during their respective protocols.
pub trait ConnectionStream {
    type Read: io::Read;
    type Write: io::Write;
    type Error: std::error::Error + Send + Sync + 'static;

    fn open(&mut self) -> Result<(&mut Self::Read, &mut Self::Write), Self::Error>;
}

/// Configuration for running [`Transport::handshake`],
/// [`Transport::ls_refs`], and [`Transport::fetch`].
pub struct Transport<S> {
    git_dir: PathBuf,
    repo: BString,
    stream: S,
}

impl<S> Transport<S>
where
    S: ConnectionStream,
{
    pub fn new(git_dir: PathBuf, repo: BString, stream: S) -> Self {
        Self {
            git_dir,
            repo,
            stream,
        }
    }

    /// Perform the handshake with the server side.
    pub fn handshake(&mut self) -> io::Result<handshake::Outcome> {
        let path = self.repo_path();
        log::debug!(target: "fetch", "performing handshake for {path}");
        let (read, write) = self.stream.open().map_err(io_other)?;
        gix_protocol::fetch::handshake(
            &mut Connection::new(read, write, FetchConnection::AllowReuse, path),
            |_| Ok(None),
            vec![],
            &mut progress::Discard,
        )
        .map_err(io_other)
    }

    /// Perform ls-refs with the server side.
    pub fn ls_refs(
        &mut self,
        mut prefixes: Vec<BString>,
        handshake: &handshake::Outcome,
    ) -> io::Result<Vec<handshake::Ref>> {
        prefixes.sort();
        prefixes.dedup();
        let path = self.repo_path();
        let (read, write) = self.stream.open().map_err(io_other)?;
        ls_refs::run(
            ls_refs::Config {
                prefixes,
                extra_params: vec![],
                repo: path.clone(),
            },
            handshake,
            Connection::new(read, write, FetchConnection::AllowReuse, path),
            &mut progress::Discard,
        )
        .map_err(io_other)
    }

    /// Perform the fetch with the server side.
    pub fn fetch(
        &mut self,
        wants: NonEmpty<ObjectId>,
        haves: Vec<ObjectId>,
        interrupt: Arc<AtomicBool>,
        handshake: &handshake::Outcome,
    ) -> io::Result<()> {
        log::debug!("running fetch wants={:?}, haves={:?}", wants, haves);
        let wants = Vec::from(wants);
        let out = {
            let wants = wants.clone();
            let path = self.repo_path();
            let (read, write) = self.stream.open().map_err(io_other)?;
            fetch::run(
                fetch::Config { wants, haves },
                fetch::PackWriter {
                    git_dir: self.git_dir.clone(),
                    interrupt,
                },
                handshake,
                Connection::new(read, write, FetchConnection::AllowReuse, path),
                &mut progress::Discard,
            )
            .map_err(io_other)?
        };
        let pack_path = out
            .pack
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "empty or no packfile received",
                )
            })?
            .index_path
            .expect("written packfile must have a path");

        // Validate we got all requested tips in the pack
        {
            use gix_pack::index::File;

            let idx = File::at(&pack_path, gix_hash::Kind::Sha1).map_err(io_other)?;
            for oid in wants {
                if idx.lookup(oid).is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("wanted {oid} not found in pack"),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Signal to the server side that we are done sending ls-refs and
    /// fetch commands.
    pub fn done(&mut self) -> io::Result<()> {
        let (_, w) = self.stream.open().map_err(io_other)?;
        w.write_all(b"heartwood/finished")
    }

    fn repo_path(&self) -> BString {
        let mut path = BString::new(b"/".to_vec());
        let mut repo = self.repo.clone();
        path.append(&mut repo);
        path
    }
}

pub(crate) struct Connection<R, W> {
    inner: client::git::Connection<R, W>,
    mode: FetchConnection,
}

impl<R, W> Connection<R, W>
where
    R: io::Read,
    W: io::Write,
{
    pub fn new(read: R, write: W, mode: FetchConnection, repo: BString) -> Self {
        Self {
            inner: client::git::Connection::new(
                read,
                write,
                Protocol::V2,
                repo,
                None::<(String, Option<u16>)>,
                client::git::ConnectMode::Daemon,
            ),
            mode,
        }
    }
}

impl<R, W> client::Transport for Connection<R, W>
where
    R: std::io::Read,
    W: std::io::Write,
{
    fn handshake<'b>(
        &mut self,
        service: Service,
        extra_parameters: &'b [(&'b str, Option<&'b str>)],
    ) -> Result<client::SetServiceResponse<'_>, client::Error> {
        self.inner.handshake(service, extra_parameters)
    }
}

impl<R, W> client::TransportWithoutIO for Connection<R, W>
where
    R: std::io::Read,
    W: std::io::Write,
{
    fn request(
        &mut self,
        write_mode: client::WriteMode,
        on_into_read: client::MessageKind,
    ) -> Result<client::RequestWriter<'_>, client::Error> {
        self.inner.request(write_mode, on_into_read)
    }

    fn to_url(&self) -> std::borrow::Cow<'_, bstr::BStr> {
        self.inner.to_url()
    }

    fn connection_persists_across_multiple_requests(&self) -> bool {
        false
    }

    fn configure(
        &mut self,
        config: &dyn std::any::Any,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        self.inner.configure(config)
    }

    fn supported_protocol_versions(&self) -> &[Protocol] {
        &[Protocol::V2]
    }
}

fn indicate_end_of_interaction<R, W>(transport: &mut Connection<R, W>) -> Result<(), client::Error>
where
    R: io::Read,
    W: io::Write,
{
    // An empty request marks the (early) end of the interaction. Only relevant in stateful transports though.
    if transport.connection_persists_across_multiple_requests() {
        transport
            .request(client::WriteMode::Binary, client::MessageKind::Flush)?
            .into_read()?;
    }
    Ok(())
}

pub fn io_other(err: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

#[derive(Debug, Error)]
pub enum WantsHavesError {
    #[error(transparent)]
    Find(#[from] refdb::error::Find),
    #[error(transparent)]
    Revwalk(#[from] odb::error::Revwalk),
}

#[derive(Clone)]
pub struct WantsHaves {
    pub wants: NonEmpty<ObjectId>,
    pub haves: Vec<ObjectId>,
}

impl WantsHaves {
    pub fn has(&self, oid: &ObjectId) -> bool {
        self.haves.contains(oid)
    }
}

#[derive(Default)]
pub struct WantsHavesBuilder {
    wants: BTreeSet<ObjectId>,
    haves: BTreeSet<ObjectId>,
}

impl WantsHavesBuilder {
    pub fn want(&mut self, oid: ObjectId) {
        self.wants.insert(oid);
    }

    pub fn have(&mut self, oid: ObjectId) {
        self.haves.insert(oid);
    }

    pub fn add<'a>(
        &mut self,
        refdb: &Refdb,
        refs: impl IntoIterator<Item = &'a ReceivedRef>,
    ) -> Result<&mut Self, WantsHavesError> {
        refs.into_iter().try_fold(self, |acc, recv| {
            let tip = recv.tip;
            let want = match refdb.refname_to_id(recv.to_qualified())? {
                Some(oid) => {
                    let want = oid != tip && !refdb.contains(tip);
                    acc.have(oid);

                    // N.b. we mark that we have the tip if oid is
                    // also an ancestor in order to not add it to our
                    // local fetch state
                    if refdb.is_in_ancestry_path(oid, tip)? {
                        acc.have(tip);
                    }

                    want
                }
                None => !refdb.contains(tip),
            };
            if want {
                acc.want(tip);
            }
            Ok(acc)
        })
    }

    pub fn build(self) -> Option<WantsHaves> {
        let wants = self
            .wants
            .into_iter()
            .filter(|want| !self.haves.contains(want));
        NonEmpty::collect(wants).map(|wants| WantsHaves {
            wants,
            haves: self.haves.into_iter().collect(),
        })
    }
}

fn agent_name() -> io::Result<String> {
    Ok(format!("git/{}", git_version()?))
}

fn git_version() -> io::Result<Version> {
    let out = std::process::Command::new("git")
        .arg("--version")
        .output()?;
    if !out.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "failed to read `git` version",
        ));
    }

    // parse: git version <version> <other optional tokens>
    out.stdout
        .split(|x| x == &b' ')
        .nth(2)
        .and_then(|s| {
            let s = std::str::from_utf8(s).ok()?;
            Version::new(s.trim())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to parse `git` version"))
}
