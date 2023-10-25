use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use git_ext::ref_format as fmt;

use crate::crypto::{Signer, Verified};
use crate::identity::doc::{Doc, DocAt, DocError, Id};
use crate::node::NodeId;

pub use crate::storage::*;

use super::fixtures;

#[derive(Clone, Debug)]
pub struct MockStorage {
    pub path: PathBuf,
    pub inventory: HashMap<Id, DocAt>,
    pub info: git::UserInfo,

    /// All refs keyed by RID.
    /// Each value is a map of refs keyed by node Id (public key).
    pub remotes: HashMap<Id, HashMap<NodeId, refs::SignedRefs<Verified>>>,
}

impl MockStorage {
    pub fn new(inventory: Vec<(Id, DocAt)>) -> Self {
        Self {
            path: PathBuf::default(),
            info: fixtures::user(),
            inventory: inventory.into_iter().collect(),
            remotes: HashMap::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Add a remote `node` with `signed_refs` for the repo `rid`.
    pub fn insert_remote(
        &mut self,
        rid: Id,
        node: NodeId,
        signed_refs: refs::SignedRefs<Verified>,
    ) {
        self.remotes
            .entry(rid)
            .or_insert(HashMap::new())
            .insert(node, signed_refs);
    }
}

impl ReadStorage for MockStorage {
    type Repository = MockRepository;

    fn info(&self) -> &git::UserInfo {
        &self.info
    }

    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn path_of(&self, rid: &Id) -> PathBuf {
        self.path().join(rid.canonical())
    }

    fn contains(&self, rid: &Id) -> Result<bool, RepositoryError> {
        Ok(self.inventory.contains_key(rid))
    }

    fn inventory(&self) -> Result<Inventory, Error> {
        Ok(self.inventory.keys().cloned().collect::<Vec<_>>())
    }

    fn repository(&self, rid: Id) -> Result<Self::Repository, Error> {
        let doc = self
            .inventory
            .get(&rid)
            .ok_or_else(|| Error::Io(io::Error::from(io::ErrorKind::NotFound)))?;
        Ok(MockRepository {
            id: rid,
            doc: doc.clone(),
            remotes: self.remotes.get(&rid).cloned().unwrap_or_default(),
        })
    }
}

impl WriteStorage for MockStorage {
    type RepositoryMut = MockRepository;

    fn repository_mut(&self, rid: Id) -> Result<Self::RepositoryMut, Error> {
        let doc = self.inventory.get(&rid).unwrap();
        Ok(MockRepository {
            id: rid,
            doc: doc.clone(),
            remotes: self.remotes.get(&rid).cloned().unwrap_or_default(),
        })
    }

    fn create(&self, _rid: Id) -> Result<Self::RepositoryMut, Error> {
        todo!()
    }

    fn remove(&self, _rid: Id) -> Result<(), Error> {
        todo!()
    }
}

#[derive(Clone, Debug)]
pub struct MockRepository {
    id: Id,
    doc: DocAt,
    remotes: HashMap<NodeId, refs::SignedRefs<Verified>>,
}

impl MockRepository {
    pub fn new(id: Id, doc: Doc<Verified>) -> Self {
        let (blob, _) = doc.encode().unwrap();

        Self {
            id,
            doc: DocAt {
                commit: Oid::from_str("ffffffffffffffffffffffffffffffffffffffff").unwrap(),
                blob,
                doc,
            },
            remotes: HashMap::default(),
        }
    }
}

impl RemoteRepository for MockRepository {
    fn remote(&self, id: &RemoteId) -> Result<Remote<Verified>, refs::Error> {
        self.remotes
            .get(id)
            .map(|refs| Remote { refs: refs.clone() })
            .ok_or(refs::Error::InvalidRef)
    }

    fn remotes(&self) -> Result<Remotes<Verified>, refs::Error> {
        Ok(self
            .remotes
            .iter()
            .map(|(id, refs)| (*id, Remote { refs: refs.clone() }))
            .collect())
    }
}

impl ValidateRepository for MockRepository {
    fn validate_remote(&self, _remote: &Remote<Verified>) -> Result<Validations, Error> {
        Ok(Validations::default())
    }
}

impl ReadRepository for MockRepository {
    fn id(&self) -> Id {
        self.id
    }

    fn is_empty(&self) -> Result<bool, git2::Error> {
        Ok(self.remotes.is_empty())
    }

    fn head(&self) -> Result<(fmt::Qualified, Oid), RepositoryError> {
        todo!()
    }

    fn canonical_head(&self) -> Result<(fmt::Qualified, Oid), RepositoryError> {
        todo!()
    }

    fn path(&self) -> &std::path::Path {
        todo!()
    }

    fn commit(&self, _oid: Oid) -> Result<git2::Commit, git_ext::Error> {
        todo!()
    }

    fn revwalk(&self, _head: Oid) -> Result<git2::Revwalk, git2::Error> {
        todo!()
    }

    fn is_ancestor_of(&self, _ancestor: Oid, _head: Oid) -> Result<bool, git_ext::Error> {
        Ok(true)
    }

    fn blob(&self, _oid: Oid) -> Result<git2::Blob, git_ext::Error> {
        todo!()
    }

    fn blob_at<P: AsRef<std::path::Path>>(
        &self,
        _oid: git_ext::Oid,
        _path: P,
    ) -> Result<git2::Blob, git_ext::Error> {
        todo!()
    }

    fn reference(
        &self,
        _remote: &RemoteId,
        _reference: &git::Qualified,
    ) -> Result<git2::Reference, git_ext::Error> {
        todo!()
    }

    fn reference_oid(
        &self,
        _remote: &RemoteId,
        _reference: &git::Qualified,
    ) -> Result<git_ext::Oid, git_ext::Error> {
        Ok(Oid::from_str("ffffffffffffffffffffffffffffffffffffffff").unwrap())
    }

    fn references_of(&self, _remote: &RemoteId) -> Result<crate::storage::refs::Refs, Error> {
        todo!()
    }

    fn references_glob(
        &self,
        _pattern: &git::PatternStr,
    ) -> Result<Vec<(fmt::Qualified, Oid)>, git::ext::Error> {
        todo!()
    }

    fn identity_doc(&self) -> Result<crate::identity::DocAt, RepositoryError> {
        Ok(self.doc.clone())
    }

    fn identity_doc_at(&self, _head: Oid) -> Result<crate::identity::DocAt, DocError> {
        Ok(self.doc.clone())
    }

    fn identity_head(&self) -> Result<Oid, RepositoryError> {
        self.canonical_identity_head()
    }

    fn identity_head_of(&self, _remote: &RemoteId) -> Result<Oid, git::ext::Error> {
        todo!()
    }

    fn identity_root(&self) -> Result<Oid, RepositoryError> {
        todo!()
    }

    fn identity_root_of(&self, _remote: &RemoteId) -> Result<Oid, RepositoryError> {
        todo!()
    }

    fn canonical_identity_head(&self) -> Result<Oid, RepositoryError> {
        Ok(Oid::from_str("cccccccccccccccccccccccccccccccccccccccc").unwrap())
    }

    fn merge_base(&self, _left: &Oid, _right: &Oid) -> Result<Oid, git::ext::Error> {
        todo!()
    }
}

impl WriteRepository for MockRepository {
    fn raw(&self) -> &git2::Repository {
        todo!()
    }

    fn set_head(&self) -> Result<Oid, RepositoryError> {
        todo!()
    }

    fn set_identity_head_to(&self, _commit: Oid) -> Result<(), RepositoryError> {
        todo!()
    }

    fn set_user(&self, _info: &git::UserInfo) -> Result<(), Error> {
        todo!()
    }
}

impl SignRepository for MockRepository {
    fn sign_refs<G: Signer>(
        &self,
        _signer: &G,
    ) -> Result<crate::storage::refs::SignedRefs<Verified>, Error> {
        todo!()
    }
}
