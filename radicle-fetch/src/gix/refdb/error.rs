use std::io;

use bstr::BString;
use git_ext::ref_format::RefString;
use gix_hash::ObjectId;
use thiserror::Error;

use crate::gix::odb;

use super::internal;

#[derive(Debug, Error)]
pub enum Init {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Open(#[from] internal::error::Open),
    #[error(transparent)]
    Snapshot(#[from] internal::error::Snapshot),
}

#[derive(Debug, Error)]
pub enum Find {
    #[error(transparent)]
    Find(#[from] gix_ref::file::find::Error),
    #[error(transparent)]
    Peeled(#[from] gix_ref::peel::to_id::Error),
}

#[derive(Debug, Error)]
pub enum RefConversion {
    #[error(transparent)]
    GixMalformed(#[from] gix_validate::refname::Error),

    #[error(transparent)]
    GitMalformed(#[from] git_ext::ref_format::Error),

    #[error("{0} unborn ref")]
    Unborn(BString),
}

#[derive(Debug, Error)]
pub enum Reload {
    #[error(transparent)]
    Snapshot(#[from] internal::error::Snapshot),
}

#[derive(Debug, Error)]
pub enum Scan {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Iter(#[from] gix_ref::file::iter::loose_then_packed::Error),
    #[error(transparent)]
    Malformed(#[from] git_ext::ref_format::Error),
    #[error(transparent)]
    Peeled(#[from] gix_ref::peel::to_id::Error),
}

#[derive(Debug, Error)]
pub enum Update {
    #[error(transparent)]
    Commit(#[from] gix_ref::file::transaction::commit::Error),
    #[error(transparent)]
    Find(#[from] Find),
    #[error(transparent)]
    Malformed(#[from] git_ext::ref_format::Error),
    #[error("missing '{0}' while preparing update")]
    Missing(RefString),
    #[error("non-fast-forward update of {name} (current: {cur}, new: {new})")]
    NonFF {
        name: BString,
        new: ObjectId,
        cur: ObjectId,
    },
    #[error(transparent)]
    Prepare(#[from] gix_ref::file::transaction::prepare::Error),
    #[error(transparent)]
    Reload(#[from] Reload),
    #[error(transparent)]
    Revwalk(#[from] odb::error::Revwalk),
    #[error("unsupported nested symref targets {0}")]
    TargetSymbolic(BString),
    #[error("rejected type change of {0}")]
    TypeChange(BString),
}
