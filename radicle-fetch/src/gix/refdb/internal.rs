use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use gix_hash::ObjectId;
use gix_ref::{
    file::{self, find, iter::LooseThenPacked, ReferenceExt, Transaction},
    packed,
    store::WriteReflog,
    PartialNameRef, Reference,
};

use crate::gix::Odb;

pub mod error {
    use super::*;
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum Open {
        #[error("failed to take a snapshot of packed-refs")]
        Snapshot(#[from] Snapshot),

        #[error(transparent)]
        Io(#[from] io::Error),
    }

    #[derive(Debug, Error)]
    pub enum Snapshot {
        #[error("failed to lock packed-refs")]
        Lock(#[from] gix_lock::acquire::Error),

        #[error("failed to open packed-refs")]
        Open(#[from] packed::buffer::open::Error),

        #[error(transparent)]
        Io(#[from] io::Error),
    }
}

pub struct Refdb {
    store: file::Store,
    buffer: Arc<RwLock<Option<Buffer>>>,
}

impl Refdb {
    pub fn open(git_dir: impl Into<PathBuf>) -> Result<Self, error::Open> {
        let store = file::Store::at(git_dir, WriteReflog::Normal, gix_hash::Kind::Sha1);
        let buffer = Arc::new(RwLock::new(Buffer::open(store.packed_refs_path())?));
        Ok(Self { store, buffer })
    }

    pub fn snapshot(&self) -> Result<Snapshot, error::Snapshot> {
        let buffer = self.buffer.read().expect("poisoned git::refdb lock");
        match &*buffer {
            Some(buffer) => {
                if buffer.is_modified()? {
                    let mtime = buffer.mtime;
                    self.reload(|buffer_| buffer_.mtime != mtime)
                } else {
                    Ok(Snapshot {
                        store: self.store.clone(),
                        buffer: Some(buffer.buf.clone()),
                    })
                }
            }
            None => {
                drop(buffer);
                self.reload(|_| true)
            }
        }
    }

    pub(crate) fn reload<F>(&self, modified_while_blocked: F) -> Result<Snapshot, error::Snapshot>
    where
        F: FnOnce(&Buffer) -> bool,
    {
        let mut write = self.buffer.write().expect("poisoned git::refdb lock");
        if let Some(buffer) = &*write {
            if modified_while_blocked(buffer) {
                return Ok(Snapshot {
                    store: self.store.clone(),
                    buffer: Some(buffer.buf.clone()),
                });
            }
        }

        match Buffer::open(self.store.packed_refs_path())? {
            Some(buffer) => {
                let buf = buffer.buf.clone();
                *write = Some(buffer);
                Ok(Snapshot {
                    store: self.store.clone(),
                    buffer: Some(buf),
                })
            }

            None => {
                *write = None;
                Ok(Snapshot {
                    store: self.store.clone(),
                    buffer: None,
                })
            }
        }
    }
}

/// A snapshot of a Git reference store paired with a [`Buffer`].
///
/// Use [`Snapshot::find`] and [`Snapshot::iter`] for finding and
/// iterating over references.
///
/// Use [`Snapshot::transaction`] for performing updates to the Git
/// reference store.
pub struct Snapshot {
    store: file::Store,
    buffer: Option<Arc<packed::Buffer>>,
}

impl Snapshot {
    pub fn find<'a, N, E>(&self, partial: N) -> Result<Option<Reference>, find::Error>
    where
        N: TryInto<&'a PartialNameRef, Error = E>,
        find::Error: From<E>,
    {
        self.store.try_find_packed(partial, self.buffer.as_deref())
    }

    pub fn iter(&self, prefix: Option<impl AsRef<Path>>) -> io::Result<LooseThenPacked> {
        let pack = self.buffer.as_deref();
        match prefix {
            None => self.store.iter_packed(pack),
            Some(prefix) => self.store.iter_prefixed_packed(prefix, pack),
        }
    }

    pub fn peel(
        &self,
        odb: &Odb,
        symref: &mut Reference,
    ) -> Result<ObjectId, gix_ref::peel::to_id::Error> {
        symref.peel_to_id_in_place_packed(
            &self.store,
            |oid, buf| {
                odb.try_find(oid, buf)
                    .map(|obj| obj.map(|o| (o.kind, o.data)))
            },
            self.buffer.as_ref().map(|buffer| buffer.as_ref()),
        )
    }

    pub fn transaction(&self) -> Transaction {
        self.store.transaction()
    }
}

/// An mmaped or in-memory buffer containing a packed-ref file.
///
/// The time of modification is kept track of to ensure safe reloading
/// of the `Snapshot`.
pub(crate) struct Buffer {
    buf: Arc<packed::Buffer>,
    path: PathBuf,
    mtime: SystemTime,
}

impl Buffer {
    fn open(path: PathBuf) -> Result<Option<Self>, error::Snapshot> {
        use gix_lock::{acquire, Marker};
        const MEM_MAP_MAX_SIZE: u64 = 32 * 1024;

        let _lock = Marker::acquire_to_hold_resource(
            &path,
            acquire::Fail::AfterDurationWithBackoff(Duration::from_millis(500)),
            None,
        )?;
        match path.metadata() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),

            Ok(meta) => {
                let mtime = meta.modified()?;
                let buf = Arc::new(packed::Buffer::open(&path, MEM_MAP_MAX_SIZE)?);
                Ok(Some(Self { buf, path, mtime }))
            }
        }
    }

    fn is_modified(&self) -> io::Result<bool> {
        match self.path.metadata() {
            // it existed before, so gone is modified
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),

            Ok(meta) => {
                let mtime = meta.modified()?;
                Ok(self.mtime == mtime)
            }
        }
    }
}
