use std::{
    io,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use gix_ref::{
    file::{self, find},
    packed,
    store::WriteReflog,
    PartialNameRef, Reference,
};

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
                    drop(buffer);
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

    pub fn reload<F>(&self, modified_while_blocked: F) -> Result<Snapshot, error::Snapshot>
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
        self.store.try_find_packed(partial, self.packed.as_deref())
    }
}

struct Buffer {
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
