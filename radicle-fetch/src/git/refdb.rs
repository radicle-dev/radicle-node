mod internal;

use radicle_crypto::PublicKey;

use crate::refdb;

#[derive(Clone)]
pub struct UserInfo {
    pub alias: String,
    pub pk: PublicKey,
}

#[derive(Clone)]
pub struct Refdb<D> {
    info: UserInfo,
    odb: D,
    refdb: internal::Refdb,
    snapshot: internal::Snapshot,
}

impl<D> Refdb<D> {
    pub fn new(info: UserInfo, odb: D, git_dir: impl Into<PathBuf>) -> Result<Self, Error> {
        let refdb = internal::Refdb::open(git_dir)?;
        let snapshot = refdb.snapshot()?;

        Ok(Self {
            info,
            odb,
            refdb,
            snapshot,
        })
    }

    pub fn reload(&mut self) -> Result<(), error::Reload> {
        self.snapshot = self.refdb.snapshot()?;
        Ok(())
    }
}
