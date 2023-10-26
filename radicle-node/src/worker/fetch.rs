pub mod error;

use std::collections::HashSet;

use radicle::crypto::PublicKey;
use radicle::git::UserInfo;
use radicle::prelude::Id;
use radicle::storage::git::Repository;
use radicle::storage::{ReadStorage as _, RefUpdate, WriteRepository as _};
use radicle::Storage;
use radicle_fetch::{BlockList, FetchLimit, Tracked};

use super::channels::ChannelsFlush;

#[derive(Debug, Default)]
pub struct FetchResult {
    /// The set of updates references.
    pub updated: Vec<RefUpdate>,
    /// The set of remote namespaces that were updated.
    pub namespaces: HashSet<PublicKey>,
}

pub enum Handle {
    Clone {
        handle: radicle_fetch::Handle<ChannelsFlush>,
        tmp: tempfile::TempDir,
    },
    Pull {
        handle: radicle_fetch::Handle<ChannelsFlush>,
    },
}

impl Handle {
    pub fn new(
        rid: Id,
        local: PublicKey,
        info: UserInfo,
        storage: &Storage,
        tracked: Tracked,
        blocked: BlockList,
        channels: ChannelsFlush,
    ) -> Result<Self, error::Handle> {
        let exists = storage.contains(&rid)?;
        if exists {
            let repo = storage.repository(rid)?;
            let handle = radicle_fetch::Handle::new(local, repo, tracked, blocked, channels)?;
            Ok(Handle::Pull { handle })
        } else {
            let tmp = tempfile::tempdir()?;
            let repo = Repository::create(tmp.path(), rid, &info)?;
            let handle = radicle_fetch::Handle::new(local, repo, tracked, blocked, channels)?;
            Ok(Handle::Clone { handle, tmp })
        }
    }

    pub fn fetch(
        self,
        rid: Id,
        storage: &Storage,
        limit: FetchLimit,
        remote: PublicKey,
    ) -> Result<FetchResult, error::Fetch> {
        let result = match self {
            Self::Clone { mut handle, tmp } => {
                log::debug!(target: "worker", "{} cloning from {remote}", handle.local());
                let result = radicle_fetch::clone(&mut handle, limit, remote)?;
                mv(tmp, storage, &rid)?;
                result
            }
            Self::Pull { mut handle } => {
                log::debug!(target: "worker", "{} pulling from {remote}", handle.local());
                radicle_fetch::pull(&mut handle, limit, remote)?
            }
        };

        for rejected in result.rejected() {
            log::warn!(target: "worker", "Rejected update for {}", rejected.refname())
        }

        for warn in result.warnings() {
            log::warn!(target: "worker", "Validation error: {}", warn);
        }

        match result {
            radicle_fetch::FetchResult::Failed { failures, .. } => {
                for fail in failures.iter() {
                    log::error!(target: "worker", "Validation error: {}", fail);
                }
                Err(error::Fetch::Validation)
            }
            radicle_fetch::FetchResult::Success {
                applied, remotes, ..
            } => {
                // N.b. We do not go through handle for this since the cloning handle
                // points to a repository that is temporary and gets moved by [`mv`].
                let repo = storage.repository(rid)?;
                repo.set_identity_head()?;
                repo.set_head()?;

                Ok(FetchResult {
                    updated: applied.updated,
                    namespaces: remotes.into_iter().collect(),
                })
            }
        }
    }
}

/// In the case of cloning, we have performed the fetch into a
/// temporary directory -- ensuring that no concurrent operations
/// see an empty repository.
///
/// At the end of the clone, we perform a rename of the temporary
/// directory to the storage repository.
///
/// # Errors
///   - Will fail if `storage` contains `rid` already.
fn mv(tmp: tempfile::TempDir, storage: &Storage, rid: &Id) -> Result<(), error::Fetch> {
    use std::io::{Error, ErrorKind};

    let from = tmp.path();
    let to = storage.path_of(rid);

    if !to.exists() {
        std::fs::rename(from, to)?;
    } else {
        log::warn!(target: "worker", "Refusing to move cloned repository {rid} already exists");
        return Err(Error::new(
            ErrorKind::AlreadyExists,
            format!("repository already exists {:?}", to),
        )
        .into());
    }

    Ok(())
}
