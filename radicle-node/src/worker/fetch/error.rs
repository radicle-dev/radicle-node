use thiserror::Error;

use radicle::{git, storage};

#[derive(Debug, Error)]
pub enum Fetch {
    #[error(transparent)]
    Run(#[from] radicle::fetch::Error),

    #[error(transparent)]
    Git(#[from] git::raw::Error),

    #[error(transparent)]
    Storage(#[from] storage::Error),

    #[error(transparent)]
    StorageCopy(#[from] std::io::Error),

    #[error(transparent)]
    Identity(#[from] radicle::identity::IdentityError),
}

#[derive(Debug, Error)]
pub enum Handle {
    #[error(transparent)]
    Context(#[from] super::context::error::Init),

    #[error(transparent)]
    Identity(#[from] radicle::identity::IdentityError),

    #[error(transparent)]
    Refdb(#[from] radicle::fetch::gix::refdb::error::Init),

    #[error(transparent)]
    Storage(#[from] radicle::storage::Error),
}
