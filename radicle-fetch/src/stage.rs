use std::collections::{BTreeMap, BTreeSet};

use gix_hash::ObjectId;
use radicle_crypto::PublicKey;
use radicle_git_ext::ref_format::RefString;

use crate::sigrefs;

// TODO: this is a stub
struct FilteredRef<T>(T);

trait Layout {
    fn pre_validate(&self, refs: &[FilteredRef<Self>]) -> Result<(), error::Layout>;
}

trait UpdateTips<T = Self> {
    fn prepare<'a, U, C>(
        &self,
        s: &FetchState<U>,
        cx: &C,
        refs: &'a [FilteredRef<T>],
    ) -> Result<Updates<'a, U>, error::Prepare>
    where
        U: ids::Urn + Ord,
        C: Identities<Urn = U>,
        for<'b> &'b C: RefScan;
}

pub struct Clone {
    pub remote: PublicKey,
    pub limit: u64,
}

impl Layout for Clone {
    fn pre_validate(&self, refs: &[FilteredRef<Self>]) -> Result<(), error::Layout> {
        verify_fetched(
            self.required_refs().collect(),
            refs.iter()
                .map(|x| refs::scoped(x.remote_id(), &self.remote_id, x.to_owned()))
                .collect(),
        )
    }
}

pub struct Fetch {
    pub local: PublicKey,
    pub remote: PublicKey,
    pub trusted: BTreeMap<PublicKey, bool>,
    pub limit: u64,
}

pub struct Refs {
    pub local: PublicKey,
    pub remote: PublicKey,
    pub trusted: sigrefs::Flattened<ObjectId>,
    pub limit: u64,
}

pub fn verify_fetched(
    expected: BTreeSet<RefString>,
    received: BTreeSet<RefString>,
) -> Result<(), error::Layout> {
    if expected.is_empty() {
        return Ok(());
    }

    let diff = expected.difference(&received).collect::<Vec<_>>();
    diff.is_empty()
        .then_some(())
        .ok_or(error::Layout::Missing(diff))
}
