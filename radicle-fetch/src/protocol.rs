use std::collections::{BTreeMap, BTreeSet};

use gix_protocol::handshake;
use radicle_crypto::{PublicKey, Signer};

use crate::gix::refdb::Updated;
use crate::gix::refdb::{Applied, SymrefTarget, Update};
use crate::sigrefs::{validation, RemoteRefs};
use crate::state::FetchState;
use crate::transport::ConnectionStream;
use crate::{identity, refs, sigrefs, stage, Handle, Identities, Tracking, Verified};

pub const FETCH_SPECIAL_LIMIT: u64 = 1024 * 1024 * 5;
pub const FETCH_REFS_LIMIT: u64 = 1024 * 1024 * 1024 * 5;

#[derive(Clone, Copy, Debug)]
pub struct FetchLimit {
    pub special: u64,
    pub refs: u64,
}

impl Default for FetchLimit {
    fn default() -> Self {
        Self {
            special: FETCH_SPECIAL_LIMIT,
            refs: FETCH_REFS_LIMIT,
        }
    }
}

pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug)]
pub struct FetchResult {
    /// The set of applied changes to the reference store.
    pub applied: Applied<'static>,
    /// The local peer keeps their own copy of `rad/id` and the fetch
    /// found a newer version of the identity.
    pub requires_confirmation: bool,
    /// Validation errors that were found while fetching.
    pub validation: Vec<validation::Validation>,
}

impl FetchResult {
    pub fn rejected(&self) -> impl Iterator<Item = &Update<'static>> {
        self.applied.rejected.iter()
    }

    pub fn updated(&self) -> impl Iterator<Item = &Updated> {
        self.applied.updated.iter()
    }

    pub fn warnings(&self) -> impl Iterator<Item = &validation::Validation> {
        self.validation.iter()
    }
}

/// If the exchange is a clone we choose to ignore the local
/// `PublicKey` since this means that the peer is attempting to clone
/// the repository fresh, and they may be a delegate of the repository
/// (e.g. they deleted their repository locally and wish to clone it
/// again).
#[derive(Clone, Copy, Debug)]
pub enum AsClone {
    Local(PublicKey),
    Keep,
}

impl AsClone {
    pub fn should_keep(&self, key: &PublicKey) -> bool {
        match self {
            Self::Local(local) => local != key,
            Self::Keep => true,
        }
    }
}

/// An overview of the exchange steps is:
///
///   1. The verification refs, i.e. `rad/id` and `rad/sigrefs`, are fetched.
///   2. Check the validity of the verification refs.
///   3. Update the identity tips.
///   4. Fetch the data references using the contents of each remote's
///      `rad/sigrefs`.
///   5. Apply any changes to the reference store.
///   6. Validate the `rad/sigrefs` and fetched contents.
///   7. Signal that the exchange is finished.
pub(crate) fn exchange<G, C, S>(
    state: &mut FetchState,
    handle: &mut Handle<G, C, S>,
    as_clone: AsClone,
    handshake: &handshake::Outcome,
    limit: FetchLimit,
    anchor: C::VerifiedIdentity,
    remote: PublicKey,
) -> Result<FetchResult, Error>
where
    G: Signer,
    C: Tracking + Identities + sigrefs::Store,
    S: ConnectionStream,
{
    let local = *handle.local();
    let delegates = anchor
        .delegates()
        .iter()
        .filter(|id| as_clone.should_keep(id))
        .copied()
        .collect::<BTreeSet<_>>();
    log::debug!(target: "fetch", "identity delegates {delegates:?}");

    let tracked = handle.context.tracked()?;
    log::debug!(target: "fetch", "tracked nodes {:?}", tracked.remotes);

    let trusted: BTreeMap<PublicKey, bool> = tracked
        .remotes
        .iter()
        .filter_map(|id| {
            if !delegates.contains(id) {
                Some((*id, false))
            } else {
                None
            }
        })
        .chain(delegates.iter().map(|id| (*id, true)))
        .collect();

    log::info!("fetching verification refs");
    let initial = stage::Fetch {
        local: as_clone,
        remote,
        delegates: delegates.clone(),
        tracked,
        limit: limit.special,
    };
    log::debug!("{initial:?}");
    state.step(handle, handshake, &initial)?;

    log::info!("loading sigrefs");
    let signed_refs = RemoteRefs::load(
        &state.as_cached(handle),
        sigrefs::Select {
            must: &delegates,
            may: &trusted
                .keys()
                .filter(|id| !delegates.contains(id))
                .copied()
                .collect(),
        },
    )?;
    log::debug!("{signed_refs:?}");

    let requires_confirmation = {
        log::info!("checking rad/ hierarchy");
        let cached = state.as_cached(handle);
        match identity::newest(&cached, &local, &delegates)? {
            None => false,
            Some((_their_id, theirs)) => {
                identity::requires_confirmation(&cached, &local, Some(anchor), theirs)?
            }
        }
    };

    log::info!("updating identity tips");
    let mut applied = {
        let pending = state.updates_mut();

        let mut tips = Vec::new();
        let mut i = 0;
        while i < pending.len() {
            match &pending[i] {
                Update::Direct { name, .. } if name.ends_with(refs::REFS_RAD_ID.as_str()) => {
                    tips.push(pending.swap_remove(i));
                }
                Update::Symbolic {
                    target: SymrefTarget { name, .. },
                    ..
                } if name.ends_with(refs::REFS_RAD_ID.as_str()) => {
                    tips.push(pending.swap_remove(i));
                }
                _ => {
                    i += 1;
                }
            }
        }
        handle.refdb.update(tips)?
    };
    log::debug!("updated tips: {applied:?}");

    state.clear_rad_refs();

    let fetch_refs = stage::Refs {
        remote,
        trusted: signed_refs,
        limit: limit.refs,
    };
    log::info!("fetching data");
    log::debug!("{fetch_refs:?}");
    state.step(handle, handshake, &fetch_refs)?;

    let signed_refs = fetch_refs.trusted;

    log::info!("updating tips");
    applied.append(&mut handle.refdb.update(state.updates_mut().drain(..))?);
    for u in &applied.updated {
        log::debug!("applied {:?}", u);
    }

    let mut warnings = Vec::new();
    log::info!("validating signed trees");
    for (remote, refs) in &signed_refs {
        let ws = validation::validate(&handle.refdb, *remote, refs)?;
        debug_assert!(
            ws.is_empty(),
            "expected no warnings for {remote}, but got {ws:?}",
        );
        warnings.extend(ws);
    }

    log::info!("validating remote trees");
    for remote in signed_refs.keys() {
        if !as_clone.should_keep(remote) {
            continue;
        }
        log::debug!("remote {}", remote);
        let refs = handle.context.load(remote)?;
        match refs {
            None => warnings.push(validation::Validation::NoData(*remote)),
            Some(refs) => {
                let ws = validation::validate(&handle.refdb, *remote, &refs)?;
                debug_assert!(
                    ws.is_empty(),
                    "expected no warnings for remote {remote}, but got {ws:?}",
                );
                warnings.extend(ws);
            }
        }
    }

    // N.b. signal to exit the upload-pack sequence
    handle.transport.done()?;

    Ok(FetchResult {
        applied,
        requires_confirmation,
        validation: warnings,
    })
}
