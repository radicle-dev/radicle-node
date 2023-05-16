use std::{io, path::Path, sync::Arc};

use gix_hash as hash;
use gix_hash::ObjectId;
use gix_object::{CommitRefIter, Kind};
use gix_odb::{store::find, store::init::Options, Find as _, Store};
use gix_traverse::commit;

pub mod error {
    use gix_hash::ObjectId;
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum Revwalk {
        #[error(transparent)]
        Find(#[from] gix_odb::store::find::Error),
        #[error("missing object {0}")]
        MissingObject(ObjectId),
        #[error(transparent)]
        Traverse(#[from] gix_traverse::commit::ancestors::Error),
    }
}

/// A handle to the Git object store.
pub struct Odb {
    pub(crate) inner: Arc<Store>,
}

/// The raw data of a Git object.
///
/// An object can be found using [`Odb::try_find`].
pub struct Object<'a> {
    pub kind: Kind,
    pub data: &'a [u8],
}

impl<'a> From<gix_object::Data<'a>> for Object<'a> {
    fn from(data: gix_object::Data<'a>) -> Self {
        Self {
            kind: data.kind,
            data: data.data,
        }
    }
}

impl Odb {
    /// Create a new [`Odb`] by providing the root path of the Git
    /// repository.
    pub fn new<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let path = dir.as_ref().join("objects");
        Ok(Self {
            inner: Arc::new(Store::at_opts(path, [], Options::default())?),
        })
    }

    /// Check if the [`Odb`] contains an object for the given `oid`.
    pub fn contains(&self, oid: impl AsRef<hash::oid>) -> bool {
        self.as_handle().contains(oid)
    }

    /// Attempt to find a Git [`Object`].
    ///
    /// If the object does not exist then `None` will be returned.
    pub fn try_find<'a>(
        &self,
        id: impl AsRef<hash::oid>,
        out: &'a mut Vec<u8>,
    ) -> Result<Option<Object<'a>>, find::Error> {
        Ok(self.as_handle().try_find(id, out)?.map(Object::from))
    }

    /// Check if `old` is an ancestor of `new`, i.e. if `old` is in
    /// the parent-chain of `new`.
    pub fn is_in_ancestry_path(
        &self,
        new: impl Into<ObjectId>,
        old: impl Into<ObjectId>,
    ) -> Result<bool, error::Revwalk> {
        let new = new.into();
        let old = old.into();

        if new == old {
            return Ok(true);
        }

        if !self.contains(new) || !self.contains(old) {
            return Ok(false);
        }

        let revwalk = commit::Ancestors::new(
            Some(new),
            commit::ancestors::State::default(),
            move |oid, buf| -> Result<CommitRefIter, error::Revwalk> {
                let obj = self
                    .try_find(oid, buf)?
                    .ok_or_else(|| error::Revwalk::MissingObject(oid.into()))?;
                Ok(CommitRefIter::from_bytes(obj.data))
            },
        );

        for parent in revwalk {
            if parent?.id == old {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn as_handle(&self) -> gix_odb::store::Handle<Arc<Store>> {
        Store::to_handle_arc(&self.inner)
    }
}
