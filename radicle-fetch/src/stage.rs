use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

use gix_hash::ObjectId;
use radicle_crypto::PublicKey;
use radicle_git_ext::ref_format::{name, refname, Namespaced, RefString};

use crate::{
    identity::Identities,
    refdb,
    refs::{self, Refname},
    sigrefs, Context,
};

fn special_refs(remote: PublicKey) -> impl Iterator<Item = Namespaced<'_>> {
    [
        refs::Refname::rad_id(remote).namespaced(),
        refs::Refname::rad_sigrefs(remote).namespaced(),
    ]
    .into_iter()
}

fn ensure_refs(
    required: BTreeSet<Namespaced<'_>>,
    wants: BTreeSet<Namespaced<'_>>,
) -> Result<(), error::Layout> {
    if wants.is_empty() {
        return Ok(());
    }

    let diff = required.difference(&wants).collect::<Vec<_>>();

    if diff.is_empty() {
        Ok(())
    } else {
        Err(error::Layout::MissingRequiredRefs(diff))
    }
}

struct ReceivedRef<T> {
    pub tip: ObjectId,
    pub name: Refname<'static>,
    _marker: PhantomData<T>,
}

impl<T> ReceivedRef<T> {
    // TODO: change docs
    /// If [`Self`] is a ref needed for verification, convert to an appropriate
    /// [`Update`].
    ///
    /// A verification ref is a [`refs::parsed::Rad`] ref, except for the
    /// [`refs::parsed::Rad::Selv`] variant which needs to be handled
    /// separately.
    pub fn as_verification_ref_update(&self) -> Option<Update> {
        use refdb::{Policy, SymrefTarget, Update};

        self.name
            .suffix
            .as_ref()
            .left()
            .and_then(|special| match special {
                Rad::Id | Rad::SignedRefs => Some(Update::Direct {
                    name: self.name.to_qualified(),
                    target: self.tip,
                    no_ff: Policy::Abort,
                }),
            })
    }
}

struct Updates {
    pub tips: Vec<refdb::Update<'a>>,
}

trait Layout {
    fn pre_validate(&self, refs: &[FilteredRef<Self>]) -> Result<(), error::Layout>;
}

trait UpdateTips<T = Self> {
    fn prepare<'a, U, C>(
        &self,
        s: &FetchState<U>,
        cx: &Context<I>,
        refs: &'a [FilteredRef<T>],
    ) -> Result<Updates<'a, U>, error::Prepare>
    where
        I: Identities;
}

pub struct Clone {
    pub remote: PublicKey,
    pub limit: u64,
}

impl Clone {
    fn required_refs(&self) -> impl Iterator<Item = Namespaced<'_>> {
        special_refs(*self.remote)
    }
}

impl Layout for Clone {
    fn pre_validate(&self, refs: &[ReceivedRef<Self>]) -> Result<(), error::Layout> {
        ensure_refs(
            self.required_refs().collect(),
            refs.iter().map(|r| r.name.namespaced()).collect(),
        )
    }
}

impl UpdateTips for Clone {
    fn prepare<'a, U, C>(
        &self,
        s: &FetchState<U>,
        cx: &Context<I>,
        refs: &'a [ReceivedRef<Self>],
    ) -> Result<Updates<'a, U>, error::Prepare>
    where
        I: Identities,
    {
        let verified = cx.identities.verified(self.id_tips().get(self.remote))?;
        let tips = if verified.delegates().contains(&self.remote) {
            refs.iter()
                .filter_map(ReceivedRef::as_verification_ref_update)
                .collect()
        } else {
            vec![]
        };

        Ok(Updates { tips })
    }
}

pub struct Fetch {
    pub local: PublicKey,
    pub remote: PublicKey,
    pub trusted: BTreeMap<PublicKey, bool>,
    pub limit: u64,
}

impl Fetch {
    pub fn required_refs(&self) -> impl Iterator<Item = Namespaced<'_>> {
        self.trusted
            .iter()
            .filter(|(id, is_delegate)| *id != self.local && is_delegate)
            .flat_map(|(id, _)| special_refs(id))
    }
}

impl Layout for Fetch {
    fn pre_validate(&self, refs: &[ReceivedRef<Self>]) -> Result<(), error::Layout> {
        ensure_refs(
            self.required_refs().collect(),
            refs.iter().map(|r| r.name.namespaced()).collect(),
        )
    }
}

impl UpdateTips for Fetch {
    fn prepare<'a, U, C>(
        &self,
        s: &FetchState<U>,
        cx: &Context<I>,
        refs: &'a [FilteredRef<Self>],
    ) -> Result<Updates<'a, U>, error::Prepare>
    where
        I: Identities,
    {
        // TODO: translate verification_refs
        prepare::verification_refs(&self.local, s, cx, refs, |remote_id| {
            self.trusted
                .get(remote_id)
                .expect("`ref_filter` yields only tracked refs")
        })
    }
}

pub struct Refs {
    pub local: PublicKey,
    pub remote: PublicKey,
    pub trusted: sigrefs::Flattened<ObjectId>,
    pub limit: u64,
}

impl UpdateTips for Refs {
    fn prepare<'a, U, C>(
        &self,
        s: &FetchState<U>,
        cx: &Context<I>,
        refs: &'a [ReceivedRef<Self>],
    ) -> Result<Updates<'a, U>, error::Prepare>
    where
        I: Identities,
    {
        let mut tips = {
            let sz = self.trusted.refs.values().map(|rs| rs.refs.len()).sum();
            Vec::with_capacity(sz)
        };

        for (remote_id, refs) in &self.trusted.refs {
            let mut signed = HashSet::with_capacity(refs.refs.len());
            for (name, tip) in refs {
                let tracking: Qualified = Qualified::from_refstr(name)
                    .map(|q| refs::Refname::remote(*remote, a).to_qualified())
                    .expect("we checked sigrefs well-formedness in wants_refs already")
                    .into();
                signed.insert(tracking.clone());
                tips.push(Update::Direct {
                    name: tracking,
                    target: tip.as_ref().to_owned(),
                    no_ff: Policy::Allow,
                });
            }

            // Prune refs not in signed
            let prefix = refname!("refs/namespaces").join(Component::from(remote_id));
            let prefix_rad = prefix.join(name::RAD);
            let scan_err = |e: <&C as RefScan>::Error| error::Prepare::Scan { source: e.into() };
            for known in cx.refdb.scan(prefix.as_str()).map_err(scan_err)? {
                let refdb::Ref { name, target, .. } = known.map_err(scan_err)?;
                // 'rad/' refs are never subject to pruning
                if name.starts_with(prefix_rad.as_str()) {
                    continue;
                }

                if !signed.contains(&name) {
                    tips.push(Update::Prune {
                        name,
                        prev: target.map_left(|oid| oid.into()),
                    });
                }
            }
        }

        Ok(Updates { tips })
    }
}

impl Layout for Refs {
    fn pre_validate(&self, refs: &[FilteredRef<Self>]) -> Result<(), error::Layout> {
        Ok(())
    }
}
