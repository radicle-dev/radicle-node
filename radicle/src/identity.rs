pub mod did;
pub mod doc;
pub mod project;

use std::collections::HashMap;

use radicle_fetch as fetch;
use radicle_git_ext::Oid;
use thiserror::Error;

use crate::crypto;
use crate::crypto::{Signature, Verified};
use crate::git;
use crate::storage;
use crate::storage::{refs, ReadRepository, RemoteId};

pub use crypto::PublicKey;
pub use did::Did;
pub use doc::{Doc, Id, IdError, PayloadError};
pub use project::Project;

/// Untrusted, well-formed input.
#[derive(Clone, Copy, Debug)]
pub struct Untrusted;
/// Signed by quorum of the previous delegation.
#[derive(Clone, Copy, Debug)]
pub struct Trusted;

#[derive(Error, Debug)]
pub enum IdentityError {
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("git: {0}")]
    GitExt(#[from] git::Error),
    #[error("identity branches diverge from each other")]
    BranchesDiverge,
    #[error("root hash `{0}` does not match project")]
    MismatchedRoot(Oid),
    #[error("the identity branch is missing")]
    MissingBranch,
    #[error("no delegate 'rad/id's were found")]
    MissingDelegateIds,
    #[error("the document root is missing")]
    MissingRoot,
    #[error("root commit is missing one or more delegate signatures")]
    MissingRootSignatures,
    #[error(transparent)]
    Payload(#[from] PayloadError),
    #[error("commit signature for {0} is invalid: {1}")]
    InvalidSignature(PublicKey, crypto::Error),
    #[error("threshold not reached: {0} signatures for a threshold of {1}")]
    ThresholdNotReached(usize, usize),
    #[error("identity document error: {0}")]
    Doc(#[from] doc::DocError),
    #[error(transparent)]
    Refs(#[from] refs::Error),
    #[error(transparent)]
    Storage(#[from] storage::Error),
}

impl IdentityError {
    /// Whether this error is caused by something not being found.
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::Doc(doc) => doc.is_not_found(),
            Self::Refs(refs) => refs.is_not_found(),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity<I> {
    /// The head of the identity branch. This points to a commit that
    /// contains the current document blob.
    pub head: Oid,
    /// The canonical identifier for this identity.
    /// This is the object id of the initial document blob.
    pub root: I,
    /// The object id of the current document blob.
    pub current: Oid,
    /// Revision number. The initial document has a revision of `0`.
    pub revision: u32,
    /// The current document.
    pub doc: Doc<Verified>,
    /// Signatures over this identity.
    pub signatures: HashMap<PublicKey, Signature>,
}

impl Identity<Oid> {
    pub fn verified(self, id: doc::Id) -> Result<Identity<doc::Id>, IdentityError> {
        // The root hash must be equal to the id.
        if self.root != *id {
            return Err(IdentityError::MismatchedRoot(self.root));
        }

        Ok(Identity {
            root: id,
            head: self.head,
            current: self.current,
            revision: self.revision,
            doc: self.doc,
            signatures: self.signatures,
        })
    }
}

impl Identity<Untrusted> {
    pub fn load<R: ReadRepository>(
        remote: &RemoteId,
        repo: &R,
    ) -> Result<Identity<Oid>, IdentityError> {
        let head = Doc::<Untrusted>::head(remote, repo)?;

        Self::load_at(head, repo)
    }

    pub fn load_at<R: ReadRepository>(head: Oid, repo: &R) -> Result<Identity<Oid>, IdentityError> {
        let mut history = repo.revwalk(head)?.collect::<Vec<_>>();

        // Retrieve root document.
        let root_oid = history.pop().ok_or(IdentityError::MissingRoot)??.into();
        let root = Doc::<Verified>::load_at(root_oid, repo)?;
        let revision = history.len() as u32;

        // Every identity founder must have signed the root document.
        for founder in &root.doc.delegates {
            if !root.sigs.iter().any(|(k, _)| k == &**founder) {
                return Err(IdentityError::MissingRootSignatures);
            }
        }

        let mut current = root.blob;
        let mut trusted = root.doc;
        let mut signatures = root.sigs;

        // Traverse the history chronologically.
        for oid in history.into_iter().rev() {
            let oid = oid?;
            let untrusted = Doc::<Verified>::load_at(oid.into(), repo)?;

            // Check that enough delegates signed this next version.
            let quorum = untrusted
                .sigs
                .iter()
                .filter(|(key, _)| trusted.delegates.iter().any(|d| **d == **key))
                .count();
            if quorum < trusted.threshold {
                return Err(IdentityError::ThresholdNotReached(
                    quorum,
                    trusted.threshold,
                ));
            }

            current = untrusted.blob;
            trusted = untrusted.doc;
            signatures = untrusted.sigs;
        }

        Ok(Identity {
            root: root.blob,
            head,
            current,
            revision,
            doc: trusted,
            signatures: signatures.into_iter().collect(),
        })
    }
}

impl fetch::Verified for Identity<doc::Id> {
    fn delegates(&self) -> nonempty::NonEmpty<PublicKey> {
        self.doc.delegates.clone().map(|did| did.into())
    }

    fn content_id(&self) -> fetch::gix::ObjectId {
        fetch::gix::oid::to_object_id(self.head)
    }

    fn revision(&self) -> fetch::gix::ObjectId {
        fetch::gix::oid::to_object_id(self.current)
    }
}

#[cfg(test)]
mod test {
    use qcheck_macros::quickcheck;
    use radicle_crypto::test::signer::MockSigner;
    use radicle_crypto::Signer as _;

    use crate::crypto::PublicKey;
    use crate::rad;
    use crate::storage::git::Storage;
    use crate::storage::{ReadStorage, WriteRepository};
    use crate::test::fixtures;

    use super::did::Did;
    use super::doc::PayloadId;
    use super::*;

    #[quickcheck]
    fn prop_json_eq_str(pk: PublicKey, proj: Id, did: Did) {
        let json = serde_json::to_string(&pk).unwrap();
        assert_eq!(format!("\"{pk}\""), json);

        let json = serde_json::to_string(&proj).unwrap();
        assert_eq!(format!("\"{}\"", proj.urn()), json);

        let json = serde_json::to_string(&did).unwrap();
        assert_eq!(format!("\"{did}\""), json);
    }

    #[test]
    fn test_valid_identity() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut rng = fastrand::Rng::new();

        let alice = MockSigner::new(&mut rng);
        let bob = MockSigner::new(&mut rng);
        let eve = MockSigner::new(&mut rng);

        let storage = Storage::open(tempdir.path().join("storage")).unwrap();
        let (id, _, _, _) =
            fixtures::project(tempdir.path().join("copy"), &storage, &alice).unwrap();

        // Bob and Eve fork the project from Alice.
        rad::fork_remote(id, alice.public_key(), &bob, &storage).unwrap();
        rad::fork_remote(id, alice.public_key(), &eve, &storage).unwrap();

        // TODO: In some cases we want to get the repo and the project, but don't
        // want to have to create a repository object twice. Perhaps there should
        // be a way of getting a project from a repo.
        let mut doc = storage.get(alice.public_key(), id).unwrap().unwrap();
        let prj = doc.project().unwrap();
        let repo = storage.repository(id).unwrap();

        // Make a change to the description and sign it.
        let desc = prj.description().to_owned() + "!";
        let prj = prj.update(None, desc, None).unwrap();
        doc.payload.insert(PayloadId::project(), prj.clone().into());
        doc.sign(&alice)
            .and_then(|(_, sig)| {
                doc.update(
                    alice.public_key(),
                    "Update description",
                    &[(alice.public_key(), sig)],
                    repo.raw(),
                )
            })
            .unwrap();

        // Add Bob as a delegate, and sign it.
        doc.delegate(bob.public_key());
        doc.threshold = 2;
        doc.sign(&alice)
            .and_then(|(_, sig)| {
                doc.update(
                    alice.public_key(),
                    "Add bob",
                    &[(alice.public_key(), sig)],
                    repo.raw(),
                )
            })
            .unwrap();

        // Add Eve as a delegate, and sign it.
        doc.delegate(eve.public_key());
        doc.sign(&alice)
            .and_then(|(_, alice_sig)| {
                doc.sign(&bob).and_then(|(_, bob_sig)| {
                    doc.update(
                        alice.public_key(),
                        "Add eve",
                        &[(alice.public_key(), alice_sig), (bob.public_key(), bob_sig)],
                        repo.raw(),
                    )
                })
            })
            .unwrap();

        // Update description again with signatures by Eve and Bob.
        let desc = prj.description().to_owned() + "?";
        let prj = prj.update(None, desc, None).unwrap();
        doc.payload.insert(PayloadId::project(), prj.into());
        let (current, head) = doc
            .sign(&bob)
            .and_then(|(_, bob_sig)| {
                doc.sign(&eve).and_then(|(blob_id, eve_sig)| {
                    doc.update(
                        alice.public_key(),
                        "Update description",
                        &[(bob.public_key(), bob_sig), (eve.public_key(), eve_sig)],
                        repo.raw(),
                    )
                    .map(|head| (blob_id, head))
                })
            })
            .unwrap();

        let identity: Identity<Id> = Identity::load(alice.public_key(), &repo)
            .unwrap()
            .verified(id)
            .unwrap();

        assert_eq!(identity.signatures.len(), 2);
        assert_eq!(identity.revision, 4);
        assert_eq!(identity.root, id);
        assert_eq!(identity.current, current);
        assert_eq!(identity.head, head);
        assert_eq!(identity.doc, doc);

        let doc = storage.get(alice.public_key(), id).unwrap().unwrap();
        assert_eq!(doc.project().unwrap().description(), "Acme's repository!?");
    }
}
