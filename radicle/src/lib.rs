#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::explicit_auto_deref)] // TODO: This can be removed when the clippy bugs are fixed
#![allow(clippy::iter_nth_zero)]

pub extern crate radicle_crypto as crypto;
pub extern crate radicle_fetch as fetch;

#[macro_use]
extern crate amplify;
extern crate radicle_git_ext as git_ext;

mod canonical;
pub mod cob;
pub mod collections;
pub mod git;
pub mod identity;
pub mod node;
pub mod profile;
pub mod rad;
pub mod serde_ext;
pub mod sql;
pub mod storage;
#[cfg(any(test, feature = "test"))]
pub mod test;
pub mod version;

pub use node::Node;
pub use profile::Profile;
pub use storage::git::Storage;

pub mod prelude {
    use super::*;

    pub use crypto::{PublicKey, Signer, Verified};
    pub use identity::{project::Project, Did, Doc, Id};
    pub use node::{Alias, NodeId, Timestamp};
    pub use profile::Profile;
    pub use storage::{
        BranchName, ReadRepository, ReadStorage, SignRepository, WriteRepository, WriteStorage,
    };
}

pub mod env {
    pub use crypto::env::*;
}
