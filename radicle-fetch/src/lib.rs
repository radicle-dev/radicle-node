use radicle_crypto::PublicKey;

mod error;

mod git;

mod identity;
use identity::Verified as VerifiedIdentity;

mod refdb;
use refdb::{Applied, Refdb};

mod odb;

mod remote;
mod sigrefs;
mod stage;
mod state;
mod transport;

mod tracking;
pub use tracking::Tracking;

mod validation;

#[macro_use]
extern crate log;
extern crate radicle_git_ext as git_ext;

// TODO: transport::Stateless in link-git will be useful for looking
// at how to implement the transport layer, which will be driven by
// radicle-node.

pub struct FetchResult {
    pub applied: Applied<'static>,
    pub requires_confirmation: bool,
    pub validation: Vec<error::Validation>,
}

pub trait Local {
    fn id(&self) -> &PublicKey;
}

#[derive(Clone, Copy, Debug)]
pub struct FetchLimit {
    pub peek: u64,
    pub data: u64,
}

impl Default for FetchLimit {
    // TODO(finto): review defaults based on how much data we expect to be
    // fetching
    fn default() -> Self {
        Self {
            peek: 1024 * 1024 * 5,
            data: 1024 * 1024 * 1024 * 5,
        }
    }
}

// pub fn pull<C>(
//     cx: &mut C,
//     limit: FetchLimit,
//     remote_id: PublicKey,
// ) -> Result<Success<<C as Identities>::Urn>, Error>
// where
//     C: Identities
//         + LocalPeer
//         + Net
//         + Refdb
//         + Odb
//         + SignedRefs<Oid = <C as Identities>::Oid>
//         + Tracking<Urn = <C as Identities>::Urn>,
//     <C as Identities>::Oid: Debug + PartialEq + Send + Sync + 'static,
//     <C as Identities>::Urn: Clone + Debug + Ord,
//     for<'a> &'a C: RefScan,
// {
//     if Local::id(cx) == &remote_id {
//         return Err("cannot replicate from self".into());
//     }
//     let anchor = ids::current(cx)?.ok_or("pull: missing `rad/id`")?;
//     eval::pull(&mut FetchState::default(), cx, limit, anchor, remote_id)
// }

// pub fn clone<C>(
//     cx: &mut C,
//     limit: FetchLimit,
//     remote_id: PublicKey,
// ) -> Result<Success<<C as Identities>::Urn>, Error>
// where
//     C: Identities
//         + LocalPeer
//         + Net
//         + Refdb
//         + Odb
//         + SignedRefs<Oid = <C as Identities>::Oid>
//         + Tracking<Urn = <C as Identities>::Urn>,
//     <C as Identities>::Oid: Debug + PartialEq + Send + Sync + 'static,
//     <C as Identities>::Urn: Clone + Debug + Ord,
//     for<'a> &'a C: RefScan,
// {
//     info!("fetching initial verification refs");
//     if Local::id(cx) == &remote_id {
//         return Err("cannot replicate from self".into());
//     }
//     let mut state = FetchState::default();
//     state.step(
//         cx,
//         &peek::ForClone {
//             remote_id,
//             limit: limit.peek,
//         },
//     )?;
//     let anchor = Identities::verify(
//         cx,
//         state
//             .id_tips()
//             .get(&remote_id)
//             .expect("BUG: peek step must ensure we got a rad/id ref"),
//         state.lookup_delegations(&remote_id),
//     )?;
//     eval::pull(&mut state, cx, limit, anchor, remote_id)
// }
