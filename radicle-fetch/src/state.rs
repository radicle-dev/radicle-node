use std::collections::{BTreeMap, BTreeSet};

use gix_protocol::handshake;
use radicle::crypto::PublicKey;
use radicle::git::{Oid, Qualified};
use radicle::identity::{Doc, DocError};

use radicle::prelude::Verified;
use radicle::storage;
use radicle::storage::{
    git::Validation, Remote, RemoteId, RemoteRepository, Remotes, ValidateRepository, Validations,
};

use crate::git;
use crate::git::refs::{Applied, Update};
use crate::git::repository;
use crate::sigrefs::SignedRefsAt;
use crate::stage;
use crate::stage::ProtocolStage;
use crate::{refs, sigrefs, transport, Handle};

/// The data size limit, 5Mb, while fetching the special refs,
/// i.e. `rad/id` and `rad/sigrefs`.
pub const DEFAULT_FETCH_SPECIAL_REFS_LIMIT: u64 = 1024 * 1024 * 5;
/// The data size limit, 5Gb, while fetching the data refs,
/// i.e. `refs/heads`, `refs/tags`, `refs/cobs`, etc.
pub const DEFAULT_FETCH_DATA_REFS_LIMIT: u64 = 1024 * 1024 * 1024 * 5;

pub mod error {
    use std::io;

    use radicle::git::Oid;
    use radicle::prelude::PublicKey;
    use thiserror::Error;

    use crate::{git, git::repository, handle, sigrefs, stage};

    #[derive(Debug, Error)]
    pub enum Step {
        #[error(transparent)]
        Io(#[from] io::Error),
        #[error(transparent)]
        Layout(#[from] stage::error::Layout),
        #[error(transparent)]
        Prepare(#[from] stage::error::Prepare),
        #[error(transparent)]
        WantsHaves(#[from] stage::error::WantsHaves),
    }

    #[derive(Debug, Error)]
    pub enum Protocol {
        #[error(transparent)]
        Ancestry(#[from] repository::error::Ancestry),
        #[error(transparent)]
        Canonical(#[from] Canonical),
        #[error("delegate '{remote}' has diverged 'rad/sigrefs': {current} -> {received}")]
        Diverged {
            remote: PublicKey,
            current: Oid,
            received: Oid,
        },
        #[error(transparent)]
        Io(#[from] io::Error),
        #[error("canonical 'refs/rad/id' is missing")]
        MissingRadId,
        #[error(transparent)]
        RefdbUpdate(#[from] repository::error::Update),
        #[error(transparent)]
        Resolve(#[from] repository::error::Resolve),
        #[error(transparent)]
        Refs(#[from] radicle::storage::refs::Error),
        #[error(transparent)]
        RemoteRefs(#[from] sigrefs::error::RemoteRefs),
        #[error(transparent)]
        Step(#[from] Step),
        #[error(transparent)]
        Tracking(#[from] handle::error::Tracking),
        #[error(transparent)]
        Validation(#[from] radicle::storage::Error),
    }

    #[derive(Debug, Error)]
    pub enum Canonical {
        #[error(transparent)]
        Resolve(#[from] git::repository::error::Resolve),
        #[error(transparent)]
        Verified(#[from] radicle::identity::DocError),
    }
}

type IdentityTips = BTreeMap<PublicKey, Oid>;
type SigrefTips = BTreeMap<PublicKey, Oid>;

#[derive(Clone, Copy, Debug)]
pub struct FetchLimit {
    pub special: u64,
    pub refs: u64,
}

impl Default for FetchLimit {
    fn default() -> Self {
        Self {
            special: DEFAULT_FETCH_SPECIAL_REFS_LIMIT,
            refs: DEFAULT_FETCH_DATA_REFS_LIMIT,
        }
    }
}

#[derive(Debug)]
pub enum FetchResult {
    Success {
        /// The set of applied changes to the reference store.
        applied: Applied<'static>,
        /// The set of namespaces that were fetched.
        remotes: BTreeSet<PublicKey>,
        /// Validation errors that were found while fetching for
        /// **non-delegate** remotes.
        warnings: sigrefs::Validations,
    },
    Failed {
        /// Validation errors that were found while fetching for
        /// **non-delegate** remotes.
        warnings: sigrefs::Validations,
        /// Validation errors that were found while fetching for
        /// **delegate** remotes.
        failures: sigrefs::Validations,
    },
}

impl FetchResult {
    pub fn rejected(&self) -> impl Iterator<Item = &Update<'static>> {
        match self {
            Self::Success { applied, .. } => either::Either::Left(applied.rejected.iter()),
            Self::Failed { .. } => either::Either::Right(std::iter::empty()),
        }
    }

    pub fn warnings(&self) -> impl Iterator<Item = &sigrefs::Validation> {
        match self {
            Self::Success { warnings, .. } => warnings.iter(),
            Self::Failed { warnings, .. } => warnings.iter(),
        }
    }

    pub fn is_success(&self) -> bool {
        match self {
            Self::Success { .. } => true,
            Self::Failed { .. } => false,
        }
    }
}

#[derive(Default)]
pub struct FetchState {
    /// In-memory refdb used to keep track of new updates without
    /// committing them to the real refdb until all validation has
    /// occurred.
    refs: git::mem::Refdb,
    /// Have we seen the `rad/id` reference?
    canonical_rad_id: Option<Oid>,
    /// Seen remote `rad/id` tips.
    ids: IdentityTips,
    /// Seen remote `rad/sigrefs` tips.
    sigrefs: SigrefTips,
    /// Seen reference tips, per remote.
    tips: BTreeMap<PublicKey, Vec<Update<'static>>>,
}

impl FetchState {
    /// Remove all tips associated with this `remote` in the
    /// `FetchState`.
    pub fn prune(&mut self, remote: &PublicKey) {
        self.ids.remove(remote);
        self.sigrefs.remove(remote);
        self.tips.remove(remote);
    }

    pub fn canonical_rad_id(&self) -> Option<&Oid> {
        self.canonical_rad_id.as_ref()
    }

    /// Update the in-memory refdb with the given updates while also
    /// keeping track of the updates in [`FetchState::tips`].
    pub fn update_all<'a, I>(&mut self, other: I) -> Applied<'a>
    where
        I: IntoIterator<Item = (PublicKey, Vec<Update<'a>>)>,
    {
        let mut ap = Applied::default();
        for (remote, ups) in other {
            for up in &ups {
                ap.append(&mut self.refs.update(Some(up.clone())));
            }
            let mut ups = ups
                .into_iter()
                .map(|up| up.into_owned())
                .collect::<Vec<_>>();
            self.tips
                .entry(remote)
                .and_modify(|tips| tips.append(&mut ups))
                .or_insert(ups);
        }
        ap
    }

    pub(crate) fn as_cached<'a, S>(&'a mut self, handle: &'a mut Handle<S>) -> Cached<'a, S> {
        Cached {
            handle,
            state: self,
        }
    }
}

impl FetchState {
    /// Perform the ls-refs and fetch for the given `step`. The result
    /// of these processes is kept track of in the internal state.
    pub(super) fn run_stage<S, F>(
        &mut self,
        handle: &mut Handle<S>,
        handshake: &handshake::Outcome,
        step: &F,
    ) -> Result<BTreeSet<PublicKey>, error::Step>
    where
        S: transport::ConnectionStream,
        F: ProtocolStage,
    {
        let refs = match step.ls_refs() {
            Some(refs) => handle
                .transport
                .ls_refs(refs.into(), handshake)?
                .into_iter()
                .filter_map(|r| step.ref_filter(r))
                .collect::<Vec<_>>(),
            None => vec![],
        };
        log::trace!(target: "fetch", "Received refs {:?}", refs);
        step.pre_validate(&refs)?;

        let wants_haves = step.wants_haves(&handle.repo, &refs)?;
        if !wants_haves.wants.is_empty() {
            handle
                .transport
                .fetch(wants_haves, handle.interrupt.clone(), handshake)?;
        } else {
            log::trace!(target: "fetch", "Nothing to fetch")
        };

        let mut fetched = BTreeSet::new();
        for r in &refs {
            match &r.name {
                refs::ReceivedRefname::Namespaced { remote, suffix } => {
                    fetched.insert(*remote);
                    if let Some(rad) = suffix.as_ref().left() {
                        match rad {
                            refs::Special::Id => {
                                self.ids.insert(*remote, r.tip);
                            }

                            refs::Special::SignedRefs => {
                                self.sigrefs.insert(*remote, r.tip);
                            }
                        }
                    }
                }
                refs::ReceivedRefname::RadId => self.canonical_rad_id = Some(r.tip),
            }
        }

        let up = step.prepare_updates(self, &handle.repo, &refs)?;
        self.update_all(up.tips.into_iter());

        Ok(fetched)
    }

    /// The finalization of the protocol exchange is as follows:
    ///
    ///   1. Load the canonical `rad/id` to use as the anchor for
    ///      getting the delegates of the identity.
    ///   2. Calculate the trusted set of peers for fetching from.
    ///   3. Fetch the special references, i.e. `rad/id` and `rad/sigrefs`.
    ///   4. Load the signed references, where these signed references
    ///      must be cryptographically verified for delegates,
    ///      otherwise they are discarded for non-delegates.
    ///   5. Fetch the data references, i.e. references found in
    ///      `rad/sigrefs`.
    ///   6. Validate the fetched references for delegates and
    ///      non-delegates, pruning any invalid remotes from the set
    ///      of updating tips.
    ///   7. Apply the valid tips, iff no delegates failed validation.
    ///   8. Signal to the other side that the process has completed.
    pub(super) fn run<S>(
        mut self,
        handle: &mut Handle<S>,
        handshake: &handshake::Outcome,
        limit: FetchLimit,
        remote: PublicKey,
    ) -> Result<FetchResult, error::Protocol>
    where
        S: transport::ConnectionStream,
    {
        // N.b. The error case here should not happen. In the case of
        // a `clone` we have asked for refs/rad/id and ensured it was
        // fetched. In the case of `pull` the repository should have
        // the refs/rad/id set.
        let anchor = self
            .as_cached(handle)
            .canonical()?
            .ok_or(error::Protocol::MissingRadId)?;

        // TODO: not sure we should allow to block *any* peer from the
        // delegate set. We could end up ignoring delegates.
        let delegates = anchor
            .delegates
            .iter()
            .filter(|id| !handle.is_blocked(id))
            .map(|did| PublicKey::from(*did))
            .collect::<BTreeSet<_>>();

        log::trace!(target: "fetch", "Identity delegates {delegates:?}");

        let tracked = handle.tracked();
        log::trace!(target: "fetch", "Tracked nodes {:?}", tracked);

        let special_refs = stage::SpecialRefs {
            blocked: handle.blocked.clone(),
            remote,
            delegates: delegates.clone(),
            tracked,
            limit: limit.special,
        };
        log::trace!(target: "fetch", "{special_refs:?}");
        let fetched = self.run_stage(handle, handshake, &special_refs)?;

        let signed_refs = sigrefs::RemoteRefs::load(
            &self.as_cached(handle),
            sigrefs::Select {
                must: &delegates,
                may: &fetched
                    .iter()
                    .filter(|id| !delegates.contains(id))
                    .copied()
                    .collect(),
            },
        )?;
        log::trace!(target: "fetch", "{signed_refs:?}");

        let data_refs = stage::DataRefs {
            remote,
            remotes: signed_refs,
            limit: limit.refs,
        };
        self.run_stage(handle, handshake, &data_refs)?;

        // Run validation of signed refs, pruning any offending
        // remotes from the tips, thus not updating the production Git
        // repository.
        // N.b. any delegate validation errors are added to
        // `failures`, while any non-delegate validation errors are
        // added to `warnings`.
        let mut warnings = sigrefs::Validations::default();
        let mut failures = sigrefs::Validations::default();
        let signed_refs = data_refs.remotes;

        for remote in signed_refs.keys() {
            if handle.is_blocked(remote) {
                continue;
            }

            let remote = sigrefs::DelegateStatus::empty(*remote, &delegates);
            match remote.load(&self.as_cached(handle))? {
                sigrefs::DelegateStatus::NonDelegate { remote, data: None } => {
                    log::debug!(target: "fetch", "Pruning non-delegate {remote} tips, missing 'rad/sigrefs'");
                    warnings.push(sigrefs::Validation::MissingRadSigRefs(remote));
                    self.prune(&remote)
                }
                sigrefs::DelegateStatus::Delegate { remote, data: None } => {
                    log::warn!(target: "fetch", "Pruning delegate {remote} tips, missing 'rad/sigrefs'");
                    failures.push(sigrefs::Validation::MissingRadSigRefs(remote));
                    self.prune(&remote)
                }
                sigrefs::DelegateStatus::NonDelegate {
                    remote,
                    data: Some(sigrefs),
                } => {
                    if let Some(SignedRefsAt { at, .. }) = SignedRefsAt::load(remote, &handle.repo)?
                    {
                        match repository::is_in_ancestry_path(&handle.repo, at, sigrefs.at)? {
                            repository::Ancestry::Ahead | repository::Ancestry::Equal => { /* do nothing */
                            }
                            repository::Ancestry::Behind | repository::Ancestry::Diverged => {
                                self.prune(&remote);
                                continue;
                            }
                        }
                    }

                    let cache = self.as_cached(handle);
                    if let Some(warns) = sigrefs::validate(&cache, sigrefs)?.as_mut() {
                        log::debug!(
                            target: "fetch",
                            "Pruning non-delegate {remote} tips, due to validation failures"
                        );
                        self.prune(&remote);
                        warnings.append(warns);
                    }
                }
                sigrefs::DelegateStatus::Delegate {
                    remote,
                    data: Some(sigrefs),
                } => {
                    if let Some(SignedRefsAt { at, .. }) = SignedRefsAt::load(remote, &handle.repo)?
                    {
                        match repository::is_in_ancestry_path(&handle.repo, at, sigrefs.at)? {
                            repository::Ancestry::Ahead | repository::Ancestry::Equal => { /* do nothing */
                            }
                            repository::Ancestry::Behind => {
                                self.prune(&remote);
                                continue;
                            }
                            repository::Ancestry::Diverged => {
                                return Err(error::Protocol::Diverged {
                                    remote,
                                    current: at,
                                    received: sigrefs.at,
                                })
                            }
                        }
                    }

                    let cache = self.as_cached(handle);
                    if let Some(fails) = sigrefs::validate(&cache, sigrefs)?.as_mut() {
                        log::warn!(target: "fetch", "Pruning delegate {remote} tips, due to validation failures");
                        self.prune(&remote);
                        failures.append(fails)
                    }
                }
            }
        }

        // N.b. signal to exit the upload-pack sequence
        handle.transport.done()?;

        // N.b. only apply to Git repository if no delegates have failed verification.
        if failures.is_empty() {
            let applied = repository::update(
                &handle.repo,
                self.tips
                    .clone()
                    .into_values()
                    .flat_map(|ups| ups.into_iter()),
            )?;
            Ok(FetchResult::Success {
                applied,
                remotes: fetched,
                warnings,
            })
        } else {
            Ok(FetchResult::Failed { warnings, failures })
        }
    }
}

/// A cached version of [`Handle`] by using the underlying
/// [`FetchState`]'s data for performing lookups.
pub(crate) struct Cached<'a, S> {
    handle: &'a mut Handle<S>,
    state: &'a mut FetchState,
}

impl<'a, S> Cached<'a, S> {
    /// Resolves `refname` to its [`ObjectId`] by first looking at the
    /// [`FetchState`] and falling back to the [`Handle::refdb`].
    pub fn refname_to_id<'b, N>(
        &self,
        refname: N,
    ) -> Result<Option<Oid>, repository::error::Resolve>
    where
        N: Into<Qualified<'b>>,
    {
        let refname = refname.into();
        match self.state.refs.refname_to_id(refname.clone()) {
            None => repository::refname_to_id(&self.handle.repo, refname),
            Some(oid) => Ok(Some(oid)),
        }
    }

    /// Get the `rad/id` found in the [`FetchState`].
    pub fn canonical_rad_id(&self) -> Option<Oid> {
        self.state.canonical_rad_id().copied()
    }

    pub fn verified(&self, head: Oid) -> Result<Doc<Verified>, DocError> {
        self.handle.verified(head)
    }

    pub fn canonical(&self) -> Result<Option<Doc<Verified>>, error::Canonical> {
        let tip = self.refname_to_id(refs::REFS_RAD_ID.clone())?;
        let cached_tip = self.canonical_rad_id();

        tip.or(cached_tip)
            .map(|tip| self.verified(tip).map_err(error::Canonical::from))
            .transpose()
    }

    pub fn load(&self, remote: &PublicKey) -> Result<Option<SignedRefsAt>, sigrefs::error::Load> {
        match self.state.sigrefs.get(remote) {
            None => SignedRefsAt::load(*remote, &self.handle.repo),
            Some(tip) => SignedRefsAt::load_at(*tip, *remote, &self.handle.repo).map(Some),
        }
    }
}

impl<'a, S> RemoteRepository for Cached<'a, S> {
    fn remote(&self, remote: &RemoteId) -> Result<Remote, storage::refs::Error> {
        // N.b. this is unused so we just delegate to the underlying
        // repository for a correct implementation.
        self.handle.repo.remote(remote)
    }

    fn remotes(&self) -> Result<Remotes<Verified>, storage::refs::Error> {
        self.state
            .sigrefs
            .keys()
            .map(|id| self.remote(id).map(|remote| (*id, remote)))
            .collect::<Result<_, _>>()
    }
}

impl<'a, S> ValidateRepository for Cached<'a, S> {
    // N.b. we don't verify the `rad/id` of each remote since they may
    // not have a reference to the COB if they have not interacted
    // with it.
    fn validate_remote(&self, remote: &Remote) -> Result<Validations, storage::Error> {
        // Contains a copy of the signed refs of this remote.
        let mut signed = BTreeMap::from((*remote.refs).clone());
        let mut validations = Validations::default();
        let mut has_sigrefs = false;

        // Check all repository references, making sure they are present in the signed refs map.
        for (refname, oid) in self.state.refs.references_of(&remote.id) {
            // Skip validation of the signed refs branch, as it is not part of `Remote`.
            if refname == storage::refs::SIGREFS_BRANCH.to_ref_string() {
                has_sigrefs = true;
                continue;
            }
            if let Some(signed_oid) = signed.remove(&refname) {
                if oid != signed_oid {
                    validations.push(Validation::MismatchedRef {
                        refname,
                        expected: signed_oid,
                        actual: oid,
                    });
                }
            } else {
                validations.push(Validation::UnsignedRef(refname));
            }
        }

        if !has_sigrefs {
            validations.push(Validation::MissingRadSigRefs(remote.id));
        }

        // The refs that are left in the map, are ones that were signed, but are not
        // in the repository. If any are left, bail.
        for (name, _) in signed.into_iter() {
            validations.push(Validation::MissingRef {
                refname: name,
                remote: remote.id,
            });
        }

        Ok(validations)
    }
}
