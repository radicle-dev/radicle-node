pub mod net;

use std::collections::BTreeMap;

use radicle_crypto::PublicKey;

use crate::refdb::RefScan;
use crate::sigrefs::{Refs, SignedRefs, Sigrefs};
use crate::state::FetchState;
use crate::{
    refdb::{SymrefTarget, Update},
    Refdb,
};
use crate::{sigrefs, stage, FetchLimit, FetchResult};
use crate::{Local, Tracking, VerifiedIdentity};

pub struct Context {}

// TODO: use concrete Refdb instead of trait
// TODO: RefScan also use concrete Refdb, I think? Perhaps not if we
//       need to use in-memory type actually
// TODO: use concrete Odb instead of trait
// TODO: consider just passing in `local: PublicKey`
pub(crate) fn exchange<C>(
    state: &mut FetchState,
    cx: &mut C,
    limit: FetchLimit,
    anchor: impl VerifiedIdentity,
    remote: PublicKey,
) -> Result<FetchResult, Error>
where
    C: Identities + Local + Net + Refdb + Odb + SignedRefs + Tracking,
    <C as Identities>::Oid: Debug + PartialEq + Send + Sync + 'static,
    for<'a> &'a C: RefScan,
{
    use either::Either::*;

    // NOTE: the shim is the FetchState + something that can drive the other traits
    let scx = state.as_shim(cx);
    let local = *Local::id(&scx);

    // NOTE: want to keep a way of getting delegates
    let delegates = VerifiedIdentity::delegates(&anchor)
        .iter()
        .filter(|id| *id != &local)
        .copied()
        .collect();

    // NOTE: we want to ask for tracked peers, but also how do we handle the Scope::All case
    let trusted: BTreeMap<PublicKey, bool> = Tracking::tracked(&scx)?
        .into_iter()
        .filter_map_ok(|id| {
            if !delegates.contains(&id) {
                Some((id, false))
            } else {
                None
            }
        })
        .chain(delegates.iter().map(|id| Ok((*id, true))))
        .collect::<Result<_, _>>()?;

    // Note: We want to keep this step
    info!("fetching verification refs");
    let peek = stage::Fetch {
        local,
        remote,
        trusted,
        limit: limit.peek,
    };
    debug!("{peek:?}");
    state.step(cx, &peek)?;

    // NOTE: we want to keep this step
    info!("loading sigrefs");
    let signed_refs = sigrefs::combined(
        &state.as_shim(cx),
        sigrefs::Select {
            must: &delegates,
            may: &peek
                .tracked
                .keys()
                .filter(|id| !delegates.contains(id))
                .copied()
                .collect(),
        },
    )?;
    debug!("{signed_refs:?}");

    // NOTE: we may or may not want this step
    let requires_confirmation = {
        info!("setting up local rad/ hierarchy");
        let shim = state.as_shim(cx);
        match ids::newest(&shim, &delegates_sans_local)? {
            None => false,
            Some((their_id, theirs)) => match rad::newer(&shim, Some(anchor), theirs)? {
                Err(error::ConfirmationRequired) => true,
                Ok(newest) => {
                    let rad::Rad { mut track, up } = match newest {
                        Left(ours) => rad::setup(&shim, None, &ours)?,
                        Right(theirs) => rad::setup(&shim, Some(their_id), &theirs)?,
                    };

                    state.trackings_mut().append(&mut track);
                    state.update_all(up);

                    false
                }
            },
        }
    };

    // NOTE: this needs to stay
    // Update identity tips already, we will only be looking at sigrefs from now
    // on. Can improve concurrency.
    info!("updating identity tips");
    let mut applied = {
        let pending = state.updates_mut();

        // `Vec::drain_filter` would help here
        let mut tips = Vec::new();
        let mut i = 0;
        while i < pending.len() {
            match &pending[i] {
                Update::Direct { name, .. } if name.ends_with(refs::name::str::REFS_RAD_ID) => {
                    tips.push(pending.swap_remove(i));
                }
                Update::Symbolic {
                    target: SymrefTarget { name, .. },
                    ..
                } if name.ends_with(refs::name::str::REFS_RAD_ID) => {
                    tips.push(pending.swap_remove(i));
                }
                _ => {
                    i += 1;
                }
            }
        }
        Refdb::update(cx, tips)?
    };

    let signed_refs = signed_refs.flattened();
    // Clear rad tips so far. Fetch will ask the remote to advertise
    // all rad refs from the transitive trackings, so we can inspect
    // the state afterwards to see if we got any.
    state.clear_rad_refs();

    let fetch = stage::Refs {
        local,
        remote,
        trusted: signed_refs,
        limit: limit.data,
    };
    info!("fetching data");
    debug!("{fetch:?}");
    state.step(cx, &fetch)?;

    let mut signed_refs = fetch.signed_refs;

    info!("updating tips");
    applied.append(&mut Refdb::update(cx, state.updates_mut().drain(..))?);
    for u in &applied.updated {
        debug!("applied {:?}", u);
    }

    // NOTE: need to look into validation code
    info!("updating signed refs");
    SignedRefs::update(cx)?;

    let mut warnings = Vec::new();
    debug!("{signed_refs:?}");
    info!("validating signed trees");
    for (peer, refs) in &signed_refs.refs {
        let ws = validation::validate::<U, _, _, _>(&*cx, peer, refs)?;
        debug_assert!(
            ws.is_empty(),
            "expected no warnings for {}, but got {:?}",
            peer,
            ws
        );
        warnings.extend(ws);
    }

    info!("validating remote trees");
    for peer in &signed_refs.remotes {
        if peer == &local {
            continue;
        }
        debug!("remote {}", peer);
        let refs = SignedRefs::load(cx, peer, 0)
            .map(|s| s.map(|Sigrefs { at, refs, .. }| Refs { at, refs }))?;
        match refs {
            None => warnings.push(error::Validation::NoData((*peer).into())),
            Some(refs) => {
                let ws = validation::validate::<U, _, _, _>(&*cx, peer, &refs)?;
                debug_assert!(
                    ws.is_empty(),
                    "expected no warnings for remote {}, but got {:?}",
                    peer,
                    ws
                );
                warnings.extend(ws);
            }
        }
    }
    Ok(FetchResult {
        applied,
        requires_confirmation,
        validation: warnings,
    })
}
