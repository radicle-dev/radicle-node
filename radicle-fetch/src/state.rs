use std::collections::BTreeMap;
use std::sync::Arc;

use gix_hash::ObjectId;
use radicle_crypto::PublicKey;

use crate::git::odb::Odb;
use crate::git::refdb::Refdb;
use crate::refdb;
use crate::transport::{self, fetch, ls_refs};
use crate::Context;

type IdentityTips = BTreeMap<PublicKey, ObjectId>;
type SigrefTips = BTreeMap<PublicKey, ObjectId>;

#[derive(Default)]
pub(crate) struct FetchState {
    refs: refdb::InMemory,
    ids: IdentityTips,
    sigs: SigrefTips,
    // tips: Vec<Update<'static>>,
}

impl FetchState {
    pub fn step<I, S>(&mut self, cx: &mut Context<I>, step: &S) -> Result<(), error::Error>
    where
        I: Identities,
        // for<'a> &'a C: RefScan,
        S: Layout + Negotiation + UpdateTips + Send + Sync + 'static,
    {
        cx.refdb.reload()?;
        let refs = match step.ls_refs() {
            None => Vec::default(),
            Some(ls) => {
                let config = transport::ls_refs::Config { prefixes: ls };
                transport::ls_refs(config, transport)?
                    .into_iter()
                    .filter_map(|r| step.ref_filter(r))
                    .collect::<Vec<_>>()
            }
        };
        Layout::pre_validate(step, &refs)?;
        match step.wants_haves(cx, &refs)? {
            Some((wants, haves)) => {
                let config = fetch::Config { wants, haves };
                transport::fetch(config, cx.connection.clone())?;
            }
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

    pub fn id_tips(&self) -> &BTreeMap<PublicKey, ObjectId> {
        &self.ids
    }
}
