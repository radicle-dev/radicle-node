use std::collections::BTreeMap;

use gix_hash::ObjectId;
use radicle_crypto::PublicKey;

use lib::refdb;

type IdentityTips = BTreeMap<PublicKey, ObjectId>;
type SigrefTips = BTreeMap<PublicKey, ObjectId>;

#[derive(Default)]
pub(crate) struct FetchState {
    refs: refdb::InMemory,
    idts: IdentityTips,
    sigs: SigrefTips,
    // tips: Vec<Update<'static>>,
}

impl FetchState {
    pub fn step<C, S>(&mut self, cx: &mut C, step: &S) -> Result<(), error::Error>
    where
        C: Identities + Net + Refdb + Odb,
        for<'a> &'a C: RefScan,
        S: Layout + Negotiation + UpdateTips + Send + Sync + 'static,
    {
        Refdb::reload(cx)?;
        let refs = match step.ls_refs() {
            None => Vec::default(),
            Some(ls) => block_on(Net::run_ls_refs(cx, ls).in_current_span())?
                .into_iter()
                .filter_map(|r| step.ref_filter(r))
                .collect::<Vec<_>>(),
        };
        Layout::pre_validate(step, &refs)?;
        match step.wants_haves(cx, &refs)? {
            Some((want, have)) => block_on(Net::run_fetch(cx, step.fetch_limit(), want, have))?,
            None => info!("nothing to fetch"),
        };

        for r in &refs {
            if let Some(rad) = r.parsed.inner.as_ref().left() {
                match rad {
                    refs::parsed::Rad::Id => {
                        self.id_tips_mut().insert(*r.remote_id(), r.tip);
                    }

                    refs::parsed::Rad::Ids { urn } => {
                        if let Ok(urn) = C::Urn::try_from_id(urn) {
                            self.delegation_tips_mut()
                                .entry(*r.remote_id())
                                .or_insert_with(BTreeMap::new)
                                .insert(urn, r.tip);
                        }
                    }

                    refs::parsed::Rad::SignedRefs => {
                        self.sigref_tips_mut().insert(*r.remote_id(), r.tip);
                    }

                    _ => {}
                }
            }
        }

        let mut up = UpdateTips::prepare(step, self, cx, &refs)?;
        self.trackings_mut().append(&mut up.track);
        self.update_all(up.tips.into_iter().map(|u| u.into_owned()));

        Ok(())
    }
}
