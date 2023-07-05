mod channels;
mod fetch;
mod upload_pack;

use std::collections::HashSet;
use std::path::PathBuf;
use std::{io, net, time};

use crossbeam_channel as chan;

use radicle::fetch::gix::refdb::UserInfo;
use radicle::fetch::FetchLimit;
use radicle::identity::Id;
use radicle::prelude::NodeId;
use radicle::storage::RefUpdate;
use radicle::{crypto, Storage};

use crate::runtime::{thread, Handle};
use crate::service::tracking;
use crate::wire::StreamId;

pub use channels::{ChannelEvent, Channels};

/// Worker pool configuration.
pub struct Config<G> {
    /// Number of worker threads.
    pub capacity: usize,
    /// Whether to use atomic fetches.
    pub atomic: bool,
    /// Timeout for all operations.
    pub timeout: time::Duration,
    /// Git daemon address.
    pub daemon: net::SocketAddr,
    /// Git storage.
    pub storage: Storage,

    pub fetch: FetchConfig<G>,
}

/// Error returned by fetch.
#[derive(thiserror::Error, Debug)]
pub enum FetchError {
    #[error("the 'git fetch' command failed with exit code '{code}'")]
    CommandFailed { code: i32 },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Fetch(#[from] fetch::error::Fetch),
    #[error(transparent)]
    Handle(#[from] fetch::error::Handle),
    #[error(transparent)]
    Identity(#[from] radicle::identity::IdentityError),
    #[error(transparent)]
    Storage(#[from] radicle::storage::Error),
    #[error(transparent)]
    Tracking(#[from] radicle::node::tracking::store::Error),
}

impl FetchError {
    /// Check if it's a timeout error.
    pub fn is_timeout(&self) -> bool {
        matches!(self, FetchError::Io(e) if e.kind() == io::ErrorKind::TimedOut)
    }
}

/// Error returned by fetch responder.
#[derive(thiserror::Error, Debug)]
pub enum UploadError {
    #[error("worker failed to connect to git daemon: {0}")]
    DaemonConnectionFailed(io::Error),
    #[error("error parsing git command packet-line: {0}")]
    PacketLine(io::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl UploadError {
    /// Check if it's an end-of-file error.
    pub fn is_eof(&self) -> bool {
        matches!(self, UploadError::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof)
    }
}

/// Fetch job sent to worker thread.
#[derive(Debug, Clone)]
pub enum FetchRequest {
    /// Client is initiating a fetch in order to receive the specified
    /// `refspecs` determined by [`Namespaces`].
    Initiator {
        /// Repo to fetch.
        rid: Id,
        /// Remote peer we are interacting with.
        remote: NodeId,
    },
    /// Server is responding to a fetch request by uploading the
    /// specified `refspecs` sent by the client.
    Responder {
        /// Remote peer we are interacting with.
        remote: NodeId,
    },
}

impl FetchRequest {
    pub fn remote(&self) -> NodeId {
        match self {
            Self::Initiator { remote, .. } | Self::Responder { remote } => *remote,
        }
    }
}

/// Fetch result of an upload or fetch.
#[derive(Debug)]
pub enum FetchResult {
    Initiator {
        /// Repo fetched.
        rid: Id,
        /// Fetch result, including remotes fetched.
        result: Result<(Vec<RefUpdate>, HashSet<NodeId>), FetchError>,
    },
    Responder {
        /// Upload result.
        result: Result<(), UploadError>,
    },
}

/// Task to be accomplished on a worker thread.
/// This is either going to be an outgoing or incoming fetch.
pub struct Task {
    pub fetch: FetchRequest,
    pub stream: StreamId,
    pub channels: Channels,
}

/// Worker response.
#[derive(Debug)]
pub struct TaskResult {
    pub remote: NodeId,
    pub result: FetchResult,
    pub stream: StreamId,
}

#[derive(Debug, Clone)]
pub struct FetchConfig<G> {
    /// Default policy, if a policy for a specific node or repository was not found.
    pub policy: tracking::Policy,
    /// Default scope, if a scope for a specific repository was not found.
    pub scope: tracking::Scope,
    /// Path to the tracking database.
    pub tracking_db: PathBuf,
    /// Data limits when fetching from a remote.
    pub limit: FetchLimit,
    /// Information of the local peer.
    pub info: UserInfo,
    /// Signer of the local peer.
    pub signer: G,
}

/// A worker that replicates git objects.
struct Worker<G> {
    nid: NodeId,
    storage: Storage,
    fetch_config: FetchConfig<G>,
    tasks: chan::Receiver<Task>,
    handle: Handle,
}

impl<G> Worker<G>
where
    G: Clone + crypto::Signer,
{
    /// Waits for tasks and runs them. Blocks indefinitely unless there is an error receiving
    /// the next task.
    fn run(mut self) -> Result<(), chan::RecvError> {
        loop {
            let task = self.tasks.recv()?;
            self.process(task);
        }
    }

    fn process(&mut self, task: Task) {
        let Task {
            fetch,
            channels,
            stream,
        } = task;
        let remote = fetch.remote();
        let tunnel = channels::Tunnel::new(self.handle.clone(), channels, remote);
        let result = self._process(fetch, stream, tunnel);

        log::trace!(target: "worker", "Sending response back to service..");

        if self
            .handle
            .worker_result(TaskResult {
                remote,
                stream,
                result,
            })
            .is_err()
        {
            log::error!(target: "worker", "Unable to report fetch result: worker channel disconnected");
        }
    }

    fn _process(
        &mut self,
        fetch: FetchRequest,
        stream: StreamId,
        mut channels: channels::Tunnel,
    ) -> FetchResult {
        match fetch {
            FetchRequest::Initiator { rid, remote } => {
                log::debug!(target: "worker", "Worker processing outgoing fetch for {}", rid);
                let result = self.fetch(rid, remote, channels);

                FetchResult::Initiator { rid, result }
            }
            FetchRequest::Responder { remote } => {
                log::debug!(target: "worker", "Worker processing incoming fetch for {remote}..");

                let (stream_r, stream_w) = channels.split();
                let (header, stream_r) = match upload_pack::header(stream_r) {
                    Ok(streams) => streams,
                    Err(e) => {
                        return FetchResult::Responder {
                            result: Err(e.into()),
                        }
                    }
                };
                let result =
                    upload_pack::upload_pack(&self.nid, &self.storage, &header, stream_r, stream_w)
                        .map(|_| ())
                        .map_err(|e| e.into());
                log::debug!(target: "worker", "Upload process on stream {stream} exited with result {result:?}");

                FetchResult::Responder { result }
            }
        }
    }

    fn fetch(
        &mut self,
        rid: Id,
        remote: NodeId,
        tunnel: channels::Tunnel,
    ) -> Result<(Vec<RefUpdate>, HashSet<NodeId>), FetchError> {
        let FetchConfig {
            policy,
            scope,
            tracking_db,
            limit,
            info,
            signer,
        } = &self.fetch_config;
        let tracking =
            tracking::Config::new(*policy, *scope, tracking::Store::reader(tracking_db)?);

        let handle = fetch::Handle::new(
            rid,
            signer.clone(),
            info.clone(),
            &self.storage,
            tracking,
            tunnel,
        )?;

        Ok(handle.fetch(rid, &self.storage, *limit, remote)?.into())
    }
}

/// A pool of workers. One thread is allocated for each worker.
pub struct Pool {
    pool: Vec<thread::JoinHandle<Result<(), chan::RecvError>>>,
}

impl Pool {
    /// Create a new worker pool with the given parameters.
    pub fn with<G>(
        tasks: chan::Receiver<Task>,
        nid: NodeId,
        handle: Handle,
        config: Config<G>,
    ) -> Self
    where
        G: crypto::Signer + Clone + Send + Sync + 'static,
    {
        let mut pool = Vec::with_capacity(config.capacity);
        for i in 0..config.capacity {
            let worker = Worker {
                nid,
                tasks: tasks.clone(),
                handle: handle.clone(),
                storage: config.storage.clone(),
                fetch_config: config.fetch.clone(),
            };
            let thread = thread::spawn(&nid, format!("worker#{i}"), || worker.run());

            pool.push(thread);
        }
        Self { pool }
    }

    /// Run the worker pool.
    ///
    /// Blocks until all worker threads have exited.
    pub fn run(self) -> thread::Result<()> {
        for (i, worker) in self.pool.into_iter().enumerate() {
            if let Err(err) = worker.join()? {
                log::trace!(target: "pool", "Worker {i} exited: {err}");
            }
        }
        log::debug!(target: "pool", "Worker pool shutting down..");

        Ok(())
    }
}

pub mod pktline {
    use std::io;
    use std::io::Read;
    use std::str;

    use super::Id;

    pub const HEADER_LEN: usize = 4;

    pub struct Reader<'a, R> {
        stream: &'a mut R,
    }

    impl<'a, R: io::Read> Reader<'a, R> {
        /// Create a new packet-line reader.
        pub fn new(stream: &'a mut R) -> Self {
            Self { stream }
        }

        /// Parse a Git request packet-line.
        ///
        /// Example: `0032git-upload-pack /project.git\0host=myserver.com\0`
        ///
        pub fn read_request_pktline(&mut self) -> io::Result<(GitRequest, Vec<u8>)> {
            let mut pktline = [0u8; 1024];
            let length = self.read_pktline(&mut pktline)?;
            let Some(cmd) = GitRequest::parse(&pktline[4..length]) else {
                return Err(io::ErrorKind::InvalidInput.into());
            };
            Ok((cmd, Vec::from(&pktline[..length])))
        }

        /// Parse a Git packet-line.
        fn read_pktline(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.read_exact(&mut buf[..HEADER_LEN])?;

            let length = str::from_utf8(&buf[..HEADER_LEN])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            let length = usize::from_str_radix(length, 16)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

            self.read_exact(&mut buf[HEADER_LEN..length])?;

            Ok(length)
        }
    }

    impl<'a, R: io::Read> io::Read for Reader<'a, R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.stream.read(buf)
        }
    }

    #[derive(Debug)]
    pub struct GitRequest {
        pub repo: Id,
        pub path: String,
        pub host: Option<(String, Option<u16>)>,
        pub extra: Vec<(String, Option<String>)>,
    }

    impl GitRequest {
        /// Parse a Git command from a packet-line.
        fn parse(input: &[u8]) -> Option<Self> {
            let input = str::from_utf8(input).ok()?;
            let mut parts = input
                .strip_prefix("git-upload-pack ")?
                .split_terminator('\0');

            let path = parts.next()?.to_owned();
            let repo = path.strip_prefix('/')?.parse().ok()?;
            let host = match parts.next() {
                None | Some("") => None,
                Some(host) => {
                    let host = host.strip_prefix("host=")?;
                    match host.split_once(':') {
                        None => Some((host.to_owned(), None)),
                        Some((host, port)) => {
                            let port = port.parse::<u16>().ok()?;
                            Some((host.to_owned(), Some(port)))
                        }
                    }
                }
            };
            let extra = parts
                .skip_while(|part| part.is_empty())
                .map(|part| match part.split_once('=') {
                    None => (part.to_owned(), None),
                    Some((k, v)) => (k.to_owned(), Some(v.to_owned())),
                })
                .collect();

            Some(Self {
                repo,
                path,
                host,
                extra,
            })
        }
    }
}
