use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Deref,
};

use git::ref_format::RefString;
use gix_hash::ObjectId;
use radicle_crypto::PublicKey;

pub mod error {
    use radicle_crypto::PublicKey;
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum Combine<E: std::error::Error + 'static> {
        #[error("required sigrefs of {0} not found")]
        NotFound(PublicKey),

        #[error(transparent)]
        Load(#[from] E),
    }
}

pub trait SignedRefs {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load the signed refs `of` remote peer, limiting the tracking graph depth
    /// to `cutoff`.
    ///
    /// The URN context is implied. `None` means the sigrefs could not be found.
    fn load(&self, of: &PublicKey) -> Result<Option<Sigrefs>, Self::Error>;

    fn load_at(&self, treeish: ObjectId, of: &PublicKey) -> Result<Option<Sigrefs>, Self::Error>;

    /// Compute and update the sigrefs for the local peer.
    ///
    /// A `None` return value denotes a no-op (ie. the sigrefs were already
    /// up-to-date).
    fn update(&self) -> Result<Option<ObjectId>, Self::Error>;
}

#[derive(Debug)]
pub struct Sigrefs {
    pub at: ObjectId,
    pub refs: HashMap<RefString, ObjectId>,
}

#[derive(Debug, Default)]
pub struct Flattened {
    /// Signed refs per tracked peer
    pub refs: BTreeMap<PublicKey, Refs>,
}

#[derive(Debug, Default)]
pub struct Combined(BTreeMap<PublicKey, Sigrefs>);

impl Combined {
    pub fn flattened(self) -> Flattened {
        let mut refs = BTreeMap::new();
        let mut remotes = BTreeSet::new();
        for (id, sigrefs) in self.0 {
            refs.insert(
                id,
                Refs {
                    at: sigrefs.at,
                    refs: sigrefs.refs,
                },
            );
        }

        Flattened { refs }
    }
}

impl Deref for Combined {
    type Target = BTreeMap<PublicKey, Sigrefs>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Combined> for Flattened {
    fn from(a: Combined) -> Self {
        a.flattened()
    }
}

impl<'a> IntoIterator for &'a Combined {
    type Item = <&'a BTreeMap<PublicKey, Sigrefs> as IntoIterator>::Item;
    type IntoIter = <&'a BTreeMap<PublicKey, Sigrefs> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug)]
pub struct Refs {
    /// Head of the `rad/signed_refs` the refs were loaded from.
    pub at: ObjectId,
    /// The signed `(refname, head)` pairs.
    pub refs: HashMap<RefString, ObjectId>,
}

impl<'a> IntoIterator for &'a Refs {
    type Item = <&'a HashMap<RefString, ObjectId> as IntoIterator>::Item;
    type IntoIter = <&'a HashMap<RefString, ObjectId> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.refs.iter()
    }
}

pub struct Select<'a> {
    pub must: &'a BTreeSet<PublicKey>,
    pub may: &'a BTreeSet<PublicKey>,
}

pub fn combined<S>(
    s: &S,
    Select { must, may }: Select,
) -> Result<Combined, error::Combine<S::Error>>
where
    S: SignedRefs,
{
    let must = must.iter().map(|id| {
        SignedRefs::load(s, id)
            .map_err(error::Combine::from)
            .and_then(|sr| match sr {
                None => Err(error::Combine::NotFound(*id)),
                Some(sr) => Ok((id, sr)),
            })
    });
    let may = may.iter().filter_map(|id| match SignedRefs::load(s, id) {
        Ok(None) => None,
        Ok(Some(sr)) => Some(Ok((id, sr))),
        Err(e) => Some(Err(e.into())),
    });

    must.chain(may)
        .fold_ok(Combined::default(), |mut acc, (id, sigrefs)| {
            acc.0.insert(*id, sigrefs);
            acc
        })
}
