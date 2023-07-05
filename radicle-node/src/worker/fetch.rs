mod context;
use context::Context;

pub mod error;

use std::collections::HashSet;
use std::path::PathBuf;

use radicle::crypto::{PublicKey, Signer};
use radicle::fetch::gix::refdb::{Updated, UserInfo};
use radicle::fetch::FetchLimit;
use radicle::git::Oid;

use radicle::prelude::{Id, NodeId};
use radicle::storage::git::Repository;

use radicle::storage::ReadStorage;
use radicle::storage::{RefUpdate, WriteRepository};
use radicle::Storage;

use crate::service;
use crate::service::tracking::store::Read;

use super::channels::Tunnel;

pub struct Handle<G> {
    inner: radicle::fetch::Handle<G, Context, Tunnel>,
    info: UserInfo,
    exists: bool,
}

impl<G: Signer> Handle<G> {
    pub fn new(
        rid: Id,
        signer: G,
        info: UserInfo,
        storage: &Storage,
        tracking: service::tracking::Config<Read>,
        tunnel: Tunnel,
    ) -> Result<Self, error::Handle> {
        let exists = storage.contains(&rid)?;
        let context = if exists {
            Context::pull(tracking, storage.repository(rid)?)
        } else {
            Context::clone(tracking, rid)?
        };
        let inner = radicle::fetch::handle(
            signer,
            context.git_dir(),
            info.clone(),
            rid.canonical().into(),
            context,
            tunnel,
        )?;

        Ok(Self {
            inner,
            info,
            exists,
        })
    }

    pub fn fetch(
        mut self,
        rid: Id,
        storage: &Storage,
        limit: FetchLimit,
        remote: PublicKey,
    ) -> Result<Updates, error::Fetch> {
        let result = if self.exists {
            log::debug!(target: "worker", "{} pulling from {remote}", self.inner.local());
            radicle::fetch::pull(&mut self.inner, limit, remote)?
        } else {
            log::debug!(target: "worker", "{} cloning from {remote}", self.inner.local());
            let res = radicle::fetch::clone(&mut self.inner, limit, remote)?;
            // N.b. we cloned into a /tmp directory and we want to now
            // move that to production storage.
            clone(storage, rid, &self.info, self.inner.context().git_dir())?;
            res
        };

        for warn in result.warnings() {
            log::warn!(target: "worker", "Validation error: {}", warn);
        }

        for rejected in result.rejected() {
            log::warn!(target: "worker", "Rejected update for {}", rejected.refname())
        }

        self.inner.context_mut().repository_mut().set_head()?;
        self.inner
            .context_mut()
            .repository_mut()
            .set_identity_head()?;

        Ok(as_ref_updates(
            self.inner.context().repository(),
            result.applied.updated,
        )?)
    }
}

#[derive(Default)]
pub struct Updates {
    pub refs: Vec<RefUpdate>,
    pub namespaces: HashSet<NodeId>,
}

impl From<Updates> for (Vec<RefUpdate>, HashSet<NodeId>) {
    fn from(Updates { refs, namespaces }: Updates) -> Self {
        (refs, namespaces)
    }
}

fn as_ref_updates(
    repo: &Repository,
    updated: impl IntoIterator<Item = Updated>,
) -> Result<Updates, radicle::git::raw::Error> {
    use radicle::fetch::gix::oid;

    updated
        .into_iter()
        .try_fold(Updates::default(), |mut updates, update| match update {
            Updated::Direct { name, prev, target } => {
                if let Some(ns) = name
                    .to_namespaced()
                    .and_then(|ns| ns.namespace().as_str().parse::<PublicKey>().ok())
                {
                    updates.namespaces.insert(ns);
                }

                match UpdateKind::new(oid::to_oid(prev), oid::to_oid(target)) {
                    UpdateKind::None => Ok(updates),
                    UpdateKind::Create(oid) => {
                        updates.refs.push(RefUpdate::Created { name, oid });
                        Ok(updates)
                    }
                    UpdateKind::Update { old, new } => {
                        updates.refs.push(RefUpdate::Updated { name, old, new });
                        Ok(updates)
                    }
                    UpdateKind::Delete(oid) => {
                        updates.refs.push(RefUpdate::Deleted { name, oid });
                        Ok(updates)
                    }
                }
            }
            Updated::Symbolic { name, prev, target } => {
                let new = repo.backend.refname_to_id(target.as_str())?;
                if let Some(ns) = name
                    .to_namespaced()
                    .and_then(|ns| ns.namespace().as_str().parse::<PublicKey>().ok())
                {
                    updates.namespaces.insert(ns);
                }
                match UpdateKind::new(oid::to_oid(prev), new.into()) {
                    UpdateKind::None => Ok(updates),
                    UpdateKind::Create(oid) => {
                        updates.refs.push(RefUpdate::Created { name, oid });
                        Ok(updates)
                    }
                    UpdateKind::Update { old, new } => {
                        updates.refs.push(RefUpdate::Updated { name, old, new });
                        Ok(updates)
                    }
                    UpdateKind::Delete(oid) => {
                        updates.refs.push(RefUpdate::Deleted { name, oid });
                        Ok(updates)
                    }
                }
            }
            Updated::Prune { name, prev } => {
                if let Some(ns) = name
                    .to_namespaced()
                    .and_then(|ns| ns.namespace().as_str().parse::<PublicKey>().ok())
                {
                    updates.namespaces.insert(ns);
                }
                updates.refs.push(RefUpdate::Deleted {
                    name,
                    oid: oid::to_oid(prev),
                });
                Ok(updates)
            }
        })
}

enum UpdateKind {
    None,
    Create(Oid),
    Update { old: Oid, new: Oid },
    Delete(Oid),
}

impl UpdateKind {
    fn new(old: Oid, new: Oid) -> Self {
        if old == new {
            Self::None
        } else if old.is_zero() {
            Self::Create(new)
        } else if new.is_zero() {
            Self::Delete(old)
        } else {
            Self::Update { old, new }
        }
    }
}

fn clone(storage: &Storage, rid: Id, info: &UserInfo, from: PathBuf) -> Result<(), error::Fetch> {
    use radicle::git::{raw, url};

    let to = storage.path_of(&rid);
    let url = url::File::new(from).to_string();

    // N.b. in the case of concurrent fetches the repository may
    // already exist. In this case, we just want to open the
    // repository and fetch the refs. This operation *should* be safe
    // since we are not forcing the refspec below.
    let repo = if to.exists() {
        raw::Repository::open(to)?
    } else {
        raw::build::RepoBuilder::new()
            .bare(true)
            .clone_local(raw::build::CloneLocal::Local)
            .clone(&url, &storage.path_of(&rid))?
    };

    {
        // The clone doesn't actually clone all refs, it only creates a ref for the
        // default branch; so we explicitly fetch the rest of the refs, so they
        // don't need to be re-fetched from the remote.
        let mut remote = repo.remote_anonymous(&url)?;
        remote.fetch(&["refs/*:refs/*"], None, None)?;
    }

    {
        let repo = storage.repository(rid)?;
        repo.set_head()?;
        repo.set_identity_head()?;
    }

    {
        let mut config = repo.config()?;

        config.set_str("user.name", &info.name())?;
        config.set_str("user.email", &info.email())?;
    }

    Ok(())
}
