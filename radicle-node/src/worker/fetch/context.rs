use std::collections::HashSet;
use std::path::PathBuf;

use radicle::fetch::gix;
use radicle::storage::git::Repository;
use thiserror::Error;

use radicle::crypto::PublicKey;
use radicle::fetch;
use radicle::fetch::{sigrefs::Store, Identities, Tracked, Tracking};
use radicle::node::tracking;
use radicle::prelude::Id;

use crate::service;
use crate::service::tracking::store::Read;

pub(super) enum Context {
    Pull {
        config: service::tracking::Config<Read>,
        repository: Repository,
    },
    Clone {
        config: service::tracking::Config<Read>,
        repository: Repository,
        _tmp: tempfile::TempDir,
    },
}

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum Init {
        #[error(transparent)]
        Storage(#[from] radicle::storage::Error),

        #[error(transparent)]
        Io(#[from] std::io::Error),
    }
}

impl Context {
    pub fn pull(config: service::tracking::Config<Read>, repository: Repository) -> Self {
        Self::Pull { config, repository }
    }

    pub fn clone(config: service::tracking::Config<Read>, rid: Id) -> Result<Self, error::Init> {
        let _tmp = tempfile::tempdir()?;
        let repository = Repository::create(_tmp.path(), rid)?;
        Ok(Self::Clone {
            config,
            repository,
            _tmp,
        })
    }

    pub fn config(&self) -> &service::tracking::Config<Read> {
        match self {
            Context::Pull { config, .. } => config,
            Context::Clone { config, .. } => config,
        }
    }

    pub fn repository_mut(&mut self) -> &mut Repository {
        match self {
            Context::Pull { repository, .. } => repository,
            Context::Clone { repository, .. } => repository,
        }
    }

    pub fn repository(&self) -> &Repository {
        match self {
            Context::Pull { repository, .. } => repository,
            Context::Clone { repository, .. } => repository,
        }
    }

    pub fn git_dir(&self) -> PathBuf {
        self.repository().backend.path().to_path_buf()
    }
}

#[derive(Debug, Error)]
pub enum TrackingError {
    #[error("Failed to find tracking policy for {rid}")]
    FailedPolicy {
        rid: Id,
        #[source]
        err: tracking::store::Error,
    },
    #[error("Cannot fetch {rid} as it is not tracked")]
    BlockedPolicy { rid: Id },
    #[error("Failed to get tracking nodes for {rid}")]
    FailedNodes {
        rid: Id,
        #[source]
        err: tracking::store::Error,
    },

    #[error(transparent)]
    Storage(#[from] radicle::storage::Error),

    #[error(transparent)]
    Git(#[from] radicle::git::raw::Error),

    #[error(transparent)]
    Refs(#[from] radicle::storage::refs::Error),
}

impl Store for Context {
    type LoadError = <Repository as Store>::LoadError;

    fn load(
        &self,
        remote: &PublicKey,
    ) -> Result<Option<radicle::fetch::sigrefs::Sigrefs>, Self::LoadError> {
        self.repository().load(remote)
    }

    fn load_at(
        &self,
        tip: impl Into<gix::ObjectId>,
        remote: &PublicKey,
    ) -> Result<Option<fetch::sigrefs::Sigrefs>, Self::LoadError> {
        self.repository().load_at(tip, remote)
    }
}

impl Identities for Context {
    type VerifiedIdentity = <Repository as Identities>::VerifiedIdentity;
    type VerifiedError = <Repository as Identities>::VerifiedError;

    fn verified(&self, head: gix::ObjectId) -> Result<Self::VerifiedIdentity, Self::VerifiedError> {
        self.repository().verified(head)
    }

    fn newer(
        &self,
        a: Self::VerifiedIdentity,
        b: Self::VerifiedIdentity,
    ) -> Result<Self::VerifiedIdentity, fetch::identity::error::History<Self::VerifiedIdentity>>
    {
        self.repository().newer(a, b)
    }
}

impl Tracking for Context {
    type Error = TrackingError;

    fn tracked(&self) -> Result<Tracked, Self::Error> {
        use TrackingError::*;

        let rid = self.repository().id;
        let entry = self
            .config()
            .repo_policy(&rid)
            .map_err(|err| FailedPolicy { rid, err })?;
        match entry.policy {
            tracking::Policy::Block => {
                log::error!(target: "service", "Attempted to fetch untracked repo {rid}");
                Err(BlockedPolicy { rid })
            }
            tracking::Policy::Track => match entry.scope {
                tracking::Scope::All => Ok(Tracked {
                    scope: fetch::Scope::All,
                    remotes: self
                        .repository()
                        .remote_ids()?
                        .collect::<Result<HashSet<_>, _>>()?,
                }),
                tracking::Scope::Trusted => {
                    let nodes = self
                        .config()
                        .node_policies()
                        .map_err(|err| FailedNodes { rid, err })?;
                    let trusted: HashSet<_> = nodes
                        .filter_map(|node| {
                            (node.policy == tracking::Policy::Track).then_some(node.id)
                        })
                        .collect();

                    Ok(Tracked {
                        scope: fetch::Scope::Trusted,
                        remotes: trusted,
                    })
                }
            },
        }
    }
}
