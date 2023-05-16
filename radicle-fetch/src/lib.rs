pub mod gix;
pub mod identity;
pub mod sigrefs;
pub mod tracking;
pub mod transport;

mod protocol;
mod refs;
mod stage;
mod state;

use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use bstr::BString;
use gix::refdb::UserInfo;
use state::FetchState;
use transport::ConnectionStream;

pub use gix::{Odb, Refdb};
pub use identity::{Identities, Verified};
pub use protocol::{FetchLimit, FetchResult};
pub use tracking::{Scope, Tracked, Tracking};
pub use transport::Transport;

use radicle_crypto::{PublicKey, Signer};
use thiserror::Error;

extern crate radicle_git_ext as git_ext;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to perform fetch handshake")]
    Handshake {
        #[source]
        err: io::Error,
    },
    #[error("failed to load `rad/id`")]
    Identity {
        #[source]
        err: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error(transparent)]
    Protocol(#[from] protocol::Error),
    #[error("missing `rad/id`")]
    MissingRadId,
    #[error("attempted to replicate from self")]
    ReplicateSelf,
}

/// The handle used for pulling or cloning changes from a remote peer.
pub struct Handle<G, C, S> {
    signer: G,
    refdb: Refdb,
    context: C,
    transport: Transport<S>,
    // Signals to the pack writer to interrupt the process
    interrupt: Arc<AtomicBool>,
}

pub fn handle<G, C, S>(
    signer: G,
    git_dir: PathBuf,
    info: UserInfo,
    repo: BString,
    context: C,
    connection: S,
) -> Result<Handle<G, C, S>, gix::refdb::error::Init>
where
    S: ConnectionStream,
    C: Tracking + Identities + sigrefs::Store,
{
    let refdb = Refdb::new(info, git_dir.clone())?;
    let transport = Transport::new(git_dir, repo, connection);

    Ok(Handle {
        signer,
        refdb,
        context,
        transport,
        interrupt: Arc::new(AtomicBool::new(false)),
    })
}

impl<G, C, S> Handle<G, C, S> {
    pub fn context(&self) -> &C {
        &self.context
    }

    pub fn local(&self) -> &PublicKey
    where
        G: Signer,
    {
        self.signer.public_key()
    }

    pub fn interrupt_pack_writer(&mut self) {
        self.interrupt.store(true, atomic::Ordering::Relaxed);
    }
}

/// Pull changes from the `remote`.
///
/// It is expected that the local peer has a copy of the repository
/// and is pulling new changes. If the repository does not exist, then
/// [`clone`] should be used.
pub fn pull<G, C, S>(
    handle: &mut Handle<G, C, S>,
    limit: FetchLimit,
    remote: PublicKey,
) -> Result<FetchResult, Error>
where
    G: Signer,
    C: Tracking + Identities + sigrefs::Store,
    S: transport::ConnectionStream,
{
    let local = *handle.local();
    if local == remote {
        return Err(Error::ReplicateSelf);
    }
    let mut state = FetchState::default();
    let anchor = identity::current(&state.as_cached(handle), &local)
        .map_err(|e| Error::Identity { err: e.into() })?
        .ok_or(Error::MissingRadId)?;
    let handshake = handle
        .transport
        .handshake()
        .map_err(|err| Error::Handshake { err })?;
    Ok(protocol::exchange(
        &mut state,
        handle,
        protocol::AsClone::Local(local),
        &handshake,
        limit,
        anchor,
        remote,
    )?)
}

/// Clone changes from the `remote`.
///
/// It is expected that the local peer has an empty repository which
/// they want to populate with the `remote`'s view of the project.
pub fn clone<G, C, S>(
    handle: &mut Handle<G, C, S>,
    limit: FetchLimit,
    remote: PublicKey,
) -> Result<FetchResult, Error>
where
    G: Signer,
    C: Tracking + Identities + sigrefs::Store,
    S: transport::ConnectionStream,
{
    log::info!("fetching initial special refs");
    if *handle.local() == remote {
        return Err(Error::ReplicateSelf);
    }
    let handshake = handle
        .transport
        .handshake()
        .map_err(|err| Error::Handshake { err })?;
    let mut state = FetchState::default();
    state
        .step(
            handle,
            &handshake,
            &stage::Clone {
                remote,
                limit: limit.special,
            },
        )
        .map_err(protocol::Error::from)?;

    let anchor = handle
        .context
        .verified(
            *state
                .canonical_id()
                .expect("missing 'rad/id' after initial clone step"),
        )
        .map_err(|e| Error::Identity { err: e.into() })?;
    Ok(protocol::exchange(
        &mut state,
        handle,
        protocol::AsClone::Keep,
        &handshake,
        limit,
        anchor,
        remote,
    )?)
}
