pub(crate) mod validation;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Deref,
};

use git_ext::ref_format::RefString;
use gix_hash::ObjectId;
use radicle_crypto::PublicKey;

pub mod error {
    use radicle_crypto::PublicKey;
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[non_exhaustive]
    pub enum RemoteRefs<E: std::error::Error + 'static> {
        #[error("required sigrefs of {0} not found")]
        NotFound(PublicKey),

        #[error(transparent)]
        Load(#[from] E),
    }
}

/// Storage for sigrefs.
pub trait Store {
    type LoadError: std::error::Error + Send + Sync + 'static;

    /// Load the signed refs of the `remote` peer.
    fn load(&self, remote: &PublicKey) -> Result<Option<Sigrefs>, Self::LoadError>;

    fn load_at(
        &self,
        tip: impl Into<ObjectId>,
        remote: &PublicKey,
    ) -> Result<Option<Sigrefs>, Self::LoadError>;
}

/// The sigrefs found for each remote.
///
/// Construct using [`RemoteRefs::load`].
#[derive(Debug, Default)]
pub struct RemoteRefs(BTreeMap<PublicKey, Sigrefs>);

impl RemoteRefs {
    /// Load the sigrefs for the given `must` and `may` remotes.
    ///
    /// The `must` remotes have to be present, otherwise an error will
    /// be returned.
    ///
    /// The `may` remotes do not have to be present and any missing
    /// sigrefs for that remote will be ignored.
    pub fn load<S>(
        store: &S,
        Select { must, may }: Select,
    ) -> Result<Self, error::RemoteRefs<S::LoadError>>
    where
        S: Store,
    {
        let must = must.iter().map(|id| {
            store
                .load(id)
                .map_err(error::RemoteRefs::from)
                .and_then(|sr| match sr {
                    None => Err(error::RemoteRefs::NotFound(*id)),
                    Some(sr) => Ok((id, sr)),
                })
        });
        let may = may.iter().filter_map(|id| match store.load(id) {
            Ok(None) => None,
            Ok(Some(sr)) => Some(Ok((id, sr))),
            Err(e) => Some(Err(e.into())),
        });

        must.chain(may)
            .try_fold(RemoteRefs::default(), |mut acc, remote_refs| {
                let (id, sigrefs) = remote_refs?;
                acc.0.insert(*id, sigrefs);
                Ok(acc)
            })
    }
}

impl Deref for RemoteRefs {
    type Target = BTreeMap<PublicKey, Sigrefs>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> IntoIterator for &'a RemoteRefs {
    type Item = <&'a BTreeMap<PublicKey, Sigrefs> as IntoIterator>::Item;
    type IntoIter = <&'a BTreeMap<PublicKey, Sigrefs> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug)]
pub struct Sigrefs {
    /// Head of the `rad/signed_refs` the refs were loaded from.
    pub at: ObjectId,
    /// The signed `(refname, head)` pairs.
    pub refs: HashMap<RefString, ObjectId>,
}

impl<'a> IntoIterator for &'a Sigrefs {
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
