use std::{collections::BTreeMap, io};

use gix_hash::ObjectId;
use gix_protocol::handshake;
use radicle_crypto::PublicKey;
use radicle_git_ext::ref_format::Qualified;
use thiserror::Error;

use crate::gix::refdb::{self, Applied, Update};
use crate::identity::Identities;
use crate::refs::RemoteRef;
use crate::stage::{error, Step};
use crate::transport::WantsHaves;
use crate::{refs, sigrefs, transport, Handle};

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Layout(#[from] error::Layout),
    #[error(transparent)]
    Prepare(#[from] error::Prepare),
    #[error(transparent)]
    Reload(#[from] refdb::error::Reload),
    #[error(transparent)]
    WantsHaves(#[from] error::WantsHaves),
}

type IdentityTips = BTreeMap<PublicKey, ObjectId>;
type SigrefTips = BTreeMap<PublicKey, ObjectId>;

#[derive(Default)]
pub struct FetchState {
    /// In-memory refdb used to keep track of new updates without
    /// committing them to the real refdb until all validation has
    /// occurred.
    refs: refdb::InMemory,
    /// Have we seen the `rad/id` reference?
    canonical_rad_id: Option<ObjectId>,
    /// Seen remote `rad/id` tips.
    ids: IdentityTips,
    /// Seen remote `rad/sigrefs` tips.
    sigrefs: SigrefTips,
    /// Seen reference tips.
    tips: Vec<Update<'static>>,
}

impl FetchState {
    pub fn canonical_id(&self) -> Option<&ObjectId> {
        self.canonical_rad_id.as_ref()
    }

    pub fn updates_mut(&mut self) -> &mut Vec<Update<'static>> {
        &mut self.tips
    }

    pub fn clear_rad_refs(&mut self) {
        self.ids.clear();
        self.sigrefs.clear();
    }

    pub fn update_all<'a, I>(&mut self, other: I) -> Applied<'a>
    where
        I: IntoIterator<Item = Update<'a>>,
    {
        let mut ap = Applied::default();
        for up in other {
            self.tips.push(up.clone().into_owned());
            ap.append(&mut self.refs.update(Some(up)));
        }
        ap
    }

    pub(crate) fn as_cached<'a, G, C, S>(
        &'a mut self,
        handle: &'a mut Handle<G, C, S>,
    ) -> Cached<'a, G, C, S> {
        Cached {
            handle,
            state: self,
        }
    }
}

impl FetchState {
    /// Perform the ls-refs and fetch for the given `step`. The result
    /// of these processes is kept track of in the internal state.
    pub(crate) fn step<G, C, S, F>(
        &mut self,
        handle: &mut Handle<G, C, S>,
        handshake: &handshake::Outcome,
        step: &F,
    ) -> Result<(), Error>
    where
        C: Identities,
        S: transport::ConnectionStream,
        F: Step,
    {
        handle.refdb.reload()?;
        let refs = match step.ls_refs() {
            Some(refs) => handle
                .transport
                .ls_refs(refs.into(), handshake)?
                .into_iter()
                .filter_map(|r| step.ref_filter(r))
                .collect::<Vec<_>>(),
            None => vec![],
        };
        log::debug!(target: "fetch", "received refs {:?}", refs);
        step.pre_validate(&refs)?;

        let wants_haves = step.wants_haves(&handle.refdb, &refs)?;
        match wants_haves.clone() {
            Some(WantsHaves { wants, haves }) => {
                handle
                    .transport
                    .fetch(wants, haves, handle.interrupt.clone(), handshake)?;
            }
            None => {
                log::info!("nothing to fetch")
            }
        };

        for r in &refs {
            if let Some(RemoteRef { remote, suffix }) = r.name.as_namespaced_ref() {
                if let Some(rad) = suffix.as_ref().left() {
                    match rad {
                        refs::Special::Id => {
                            // Only add to `ids` if we have not seen this tip before
                            if wants_haves
                                .as_ref()
                                .map(|x| !x.has(&r.tip))
                                .unwrap_or(false)
                            {
                                self.ids.insert(*remote, r.tip);
                            }
                        }

                        refs::Special::SignedRefs => {
                            // Only add to `sigrefs` if we have not seen this tip before
                            if wants_haves
                                .as_ref()
                                .map(|x| !x.has(&r.tip))
                                .unwrap_or(false)
                            {
                                self.sigrefs.insert(*remote, r.tip);
                            }
                        }
                    }
                }
            } else {
                self.canonical_rad_id = Some(r.tip);
            }
        }

        let up = step.prepare(self, &handle.refdb, &handle.context, &refs)?;
        self.update_all(up.tips.into_iter().map(|u| u.into_owned()));

        Ok(())
    }
}

/// A cached version of [`Handle`] by using the underlying
/// [`FetchState`]'s data for performing lookups.
pub(crate) struct Cached<'a, G, C, S> {
    handle: &'a mut Handle<G, C, S>,
    state: &'a mut FetchState,
}

impl<'a, G, C, S> Cached<'a, G, C, S> {
    /// Resolves `refname` to its [`ObjectId`] by first looking at the
    /// [`FetchState`] and falling back to the [`Handle::refdb`].
    pub fn refname_to_id<'b, N>(&self, refname: N) -> Result<Option<ObjectId>, refdb::error::Find>
    where
        N: Into<Qualified<'b>>,
    {
        let refname = refname.into();
        match self.state.refs.refname_to_id(refname.clone()) {
            None => self.handle.refdb.refname_to_id(refname),
            Some(oid) => Ok(Some(oid)),
        }
    }

    /// Get the `rad/id` found in the [`FetchState`].
    pub fn canonical_rad_id(&self) -> Option<ObjectId> {
        self.state.canonical_id().copied()
    }
}

// N.b. only checks the `FetchState` for `load` and always uses
// `Handle` for `load_at`.
impl<'a, G, C, S> sigrefs::Store for Cached<'a, G, C, S>
where
    C: sigrefs::Store,
{
    type LoadError = C::LoadError;

    fn load(&self, remote: &PublicKey) -> Result<Option<sigrefs::Sigrefs>, Self::LoadError> {
        match self.state.sigrefs.get(remote) {
            None => self.handle.context.load(remote),
            Some(tip) => self.handle.context.load_at(*tip, remote),
        }
    }

    fn load_at(
        &self,
        tip: impl Into<ObjectId>,
        remote: &PublicKey,
    ) -> Result<Option<sigrefs::Sigrefs>, Self::LoadError> {
        self.handle.context.load_at(tip, remote)
    }
}

// N.b. always uses the `Handle` for resolving identities.
impl<'a, G, C, S> Identities for Cached<'a, G, C, S>
where
    C: Identities,
{
    type VerifiedIdentity = C::VerifiedIdentity;
    type VerifiedError = C::VerifiedError;

    fn verified(&self, head: ObjectId) -> Result<Self::VerifiedIdentity, Self::VerifiedError> {
        self.handle.context.verified(head)
    }

    fn newer(
        &self,
        a: Self::VerifiedIdentity,
        b: Self::VerifiedIdentity,
    ) -> Result<Self::VerifiedIdentity, crate::identity::error::History<Self::VerifiedIdentity>>
    {
        self.handle.context.newer(a, b)
    }
}
