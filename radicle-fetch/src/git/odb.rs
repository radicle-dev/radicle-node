use std::path::Path;

use gix_hash as hash;
use gix_object::Kind;
use gix_odb::{
    loose::{find, Store},
    Cache,
};

pub struct Odb {
    inner: Store,
}

pub struct Object<'a> {
    pub kind: Kind,
    pub data: &'a [u8],
}

impl Odb {
    pub fn new<P: AsRef<Path>>(dir: P) -> Self {
        Self {
            inner: Store::at(dir.as_ref().to_path_buf(), hash::Kind::Sha1),
        }
    }

    pub fn contains(&self, oid: impl AsRef<hash::oid>) -> bool {
        self.inner.contains(oid)
    }

    pub fn try_find<'a>(
        &self,
        id: impl AsRef<hash::oid>,
        out: &'a mut Vec<u8>,
    ) -> Result<Option<Object<'a>>, find::Error> {
        let data = self.inner.try_find(id, out)?;
        data.map(|data| {
            Ok(Object {
                kind: data.kind,
                data: data.data,
            })
        })
        .transpose()
    }
}
