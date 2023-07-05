pub mod cob;
pub mod transport;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::{fs, io};

use crypto::{Signer, Unverified, Verified};
use once_cell::sync::Lazy;
use radicle_fetch as fetch;

use crate::git;
use crate::identity;
use crate::identity::doc::DocError;
use crate::identity::{Doc, Id};
use crate::identity::{Identity, IdentityError, Project};
use crate::storage::refs;
use crate::storage::refs::{Refs, SignedRefs};
use crate::storage::{
    Inventory, ReadRepository, ReadStorage, Remote, Remotes, SignRepository, WriteRepository,
    WriteStorage,
};

pub use crate::git::*;
pub use crate::storage::Error;

use super::RemoteId;

pub static NAMESPACES_GLOB: Lazy<git::refspec::PatternString> =
    Lazy::new(|| git::refspec::pattern!("refs/namespaces/*"));
pub static SIGREFS_GLOB: Lazy<refspec::PatternString> =
    Lazy::new(|| git::refspec::pattern!("refs/namespaces/*/rad/sigrefs"));
pub static CANONICAL_IDENTITY: Lazy<git::Qualified> = Lazy::new(|| {
    git::Qualified::from_components(
        git::name::component!("rad"),
        git::name::component!("id"),
        None,
    )
});

/// A parsed Git reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub oid: git::Oid,
    pub name: RefString,
    pub namespace: Option<RemoteId>,
}

impl<'a> TryFrom<git2::Reference<'a>> for Ref {
    type Error = RefError;

    fn try_from(r: git2::Reference) -> Result<Self, Self::Error> {
        let name = r.name().ok_or(RefError::InvalidName)?;
        let (namespace, name) = match git::parse_ref_namespaced::<RemoteId>(name) {
            Ok((namespace, refname)) => (Some(namespace), refname.to_ref_string()),
            Err(RefError::MissingNamespace(refname)) => (None, refname),
            Err(err) => return Err(err),
        };
        let Some(oid) = r.target() else {
            // Ignore symbolic refs, eg. `HEAD`.
            return Err(RefError::Symbolic(name));
        };
        Ok(Self {
            namespace,
            name,
            oid: oid.into(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Storage {
    path: PathBuf,
}

impl ReadStorage for Storage {
    type Repository = Repository;

    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn path_of(&self, rid: &Id) -> PathBuf {
        paths::repository(&self, rid)
    }

    fn contains(&self, rid: &Id) -> Result<bool, IdentityError> {
        if paths::repository(&self, rid).exists() {
            let _ = self.repository(*rid)?.head()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn get(&self, remote: &RemoteId, proj: Id) -> Result<Option<Doc<Verified>>, IdentityError> {
        let repo = match self.repository(proj) {
            Ok(doc) => doc,
            Err(e) if e.is_not_found() => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match repo.identity_doc_of(remote) {
            Ok(doc) => Ok(Some(doc)),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn inventory(&self) -> Result<Inventory, Error> {
        self.repositories()
    }

    fn repository(&self, rid: Id) -> Result<Self::Repository, Error> {
        Repository::open(paths::repository(self, &rid), rid)
    }
}

impl WriteStorage for Storage {
    type RepositoryMut = Repository;

    fn repository_mut(&self, rid: Id) -> Result<Self::RepositoryMut, Error> {
        Repository::open(paths::repository(self, &rid), rid)
    }

    fn create(&self, rid: Id) -> Result<Self::RepositoryMut, Error> {
        Repository::create(paths::repository(self, &rid), rid)
    }
}

impl Storage {
    // TODO: Return a better error when not found.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, io::Error> {
        let path = path.as_ref().to_path_buf();

        match fs::create_dir_all(&path) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
            Ok(()) => {}
        }

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn repositories(&self) -> Result<Vec<Id>, Error> {
        let mut repos = Vec::new();

        for result in fs::read_dir(&self.path)? {
            let path = result?;

            // Skip non-directories.
            if !path.file_type()?.is_dir() {
                continue;
            }
            // Skip hidden files.
            if path.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let rid =
                Id::try_from(path.file_name()).map_err(|_| Error::InvalidId(path.file_name()))?;
            let repo = self.repository(rid)?;

            // For performance reasons, we don't do a full repository check here.
            if let Err(e) = repo.head() {
                log::warn!(target: "storage", "Repository {rid} is invalid: looking up head: {e}");
                continue;
            }
            repos.push(rid);
        }
        Ok(repos)
    }

    pub fn inspect(&self) -> Result<(), Error> {
        for proj in self.repositories()? {
            let repo = self.repository(proj)?;

            for r in repo.raw().references()? {
                let r = r?;
                let name = r.name().ok_or(Error::InvalidRef)?;
                let oid = r.target().ok_or(Error::InvalidRef)?;

                println!("{} {oid} {name}", proj.urn());
            }
        }
        Ok(())
    }
}

pub struct Repository {
    pub id: Id,
    pub backend: git2::Repository,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("invalid remote `{0}`")]
    InvalidRemote(RemoteId),
    #[error("invalid target `{2}` for reference `{1}` of remote `{0}`")]
    InvalidRefTarget(RemoteId, RefString, git2::Oid),
    #[error("invalid identity: {0}")]
    InvalidIdentity(#[from] IdentityError),
    #[error("refs error: {0}")]
    Refs(#[from] refs::Error),
    #[error("missing reference `{1}` in remote `{0}`")]
    MissingRef(RemoteId, git::RefString),
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Storage(#[from] Error),
}

impl Repository {
    /// Open an existing repository.
    pub fn open<P: AsRef<Path>>(path: P, id: Id) -> Result<Self, Error> {
        let backend = git2::Repository::open_bare(path.as_ref())?;

        Ok(Self { id, backend })
    }

    /// Create a new repository.
    pub fn create<P: AsRef<Path>>(path: P, id: Id) -> Result<Self, Error> {
        let backend = git2::Repository::init_opts(
            &path,
            git2::RepositoryInitOptions::new()
                .bare(true)
                .no_reinit(true)
                .external_template(false),
        )?;
        let mut config = backend.config()?;

        // TODO: Get ahold of user name and/or key.
        config.set_str("user.name", "radicle")?;
        config.set_str("user.email", "radicle@localhost")?;

        Ok(Self { id, backend })
    }

    /// Create the repository's identity branch.
    pub fn init<G: Signer, S: WriteStorage>(
        doc: &Doc<Verified>,
        remote: &RemoteId,
        storage: S,
        signer: &G,
    ) -> Result<(Self, git::Oid), Error> {
        let (doc_oid, doc) = doc.encode()?;
        let id = Id::from(doc_oid);
        let repo = Self::create(paths::repository(&storage, &id), id)?;
        let oid = Doc::init(
            doc.as_slice(),
            remote,
            &[(signer.public_key(), signer.sign(doc_oid.as_bytes()))],
            repo.raw(),
        )?;

        Ok((repo, oid))
    }

    pub fn inspect(&self) -> Result<(), Error> {
        for r in self.backend.references()? {
            let r = r?;
            let name = r.name().ok_or(Error::InvalidRef)?;
            let oid = r.target().ok_or(Error::InvalidRef)?;

            println!("{oid} {name}");
        }
        Ok(())
    }

    /// Iterate over all references.
    pub fn references(
        &self,
    ) -> Result<impl Iterator<Item = Result<Ref, refs::Error>> + '_, git2::Error> {
        let refs = self
            .backend
            .references()?
            .map(|reference| {
                let r = reference?;

                match Ref::try_from(r) {
                    Err(RefError::Symbolic(_)) => Ok(None),
                    Err(err) => Err(err.into()),
                    Ok(r) => Ok(Some(r)),
                }
            })
            .filter_map(Result::transpose);

        Ok(refs)
    }

    pub fn identity_of(&self, remote: &RemoteId) -> Result<Identity<Oid>, IdentityError> {
        Identity::load(remote, self)
    }

    /// Get the canonical project information.
    pub fn project(&self) -> Result<Project, IdentityError> {
        let head = self.identity_head()?;
        let doc = self.identity_doc_at(head)?;
        let proj = doc.verified()?.project()?;

        Ok(proj)
    }

    pub fn identity_doc_of(&self, remote: &RemoteId) -> Result<Doc<Verified>, IdentityError> {
        let (doc, _) = identity::Doc::load(remote, self)?;
        let verified = doc.verified()?;

        Ok(verified)
    }

    pub fn remote_ids(
        &self,
    ) -> Result<impl Iterator<Item = Result<RemoteId, refs::Error>> + '_, git2::Error> {
        let iter = self.backend.references_glob(SIGREFS_GLOB.as_str())?.map(
            |reference| -> Result<RemoteId, refs::Error> {
                let r = reference?;
                let name = r.name().ok_or(refs::Error::InvalidRef)?;
                let (id, _) = git::parse_ref_namespaced::<RemoteId>(name)?;

                Ok(id)
            },
        );
        Ok(iter)
    }

    pub fn remotes(
        &self,
    ) -> Result<
        impl Iterator<Item = Result<(RemoteId, Remote<Verified>), refs::Error>> + '_,
        git2::Error,
    > {
        let remotes =
            self.backend
                .references_glob(SIGREFS_GLOB.as_str())?
                .map(|reference| -> Result<_, _> {
                    let r = reference?;
                    let name = r.name().ok_or(refs::Error::InvalidRef)?;
                    let (id, _) = git::parse_ref_namespaced::<RemoteId>(name)?;
                    let remote = self.remote(&id)?;

                    Ok((id, remote))
                });
        Ok(remotes)
    }
}

impl ReadRepository for Repository {
    fn id(&self) -> Id {
        self.id
    }

    fn is_empty(&self) -> Result<bool, git2::Error> {
        Ok(self.remotes()?.next().is_none())
    }

    fn path(&self) -> &Path {
        self.backend.path()
    }

    fn blob_at<'a>(&'a self, oid: Oid, path: &'a Path) -> Result<git2::Blob<'a>, git::Error> {
        git::ext::Blob::At {
            object: oid.into(),
            path,
        }
        .get(&self.backend)
    }

    fn validate_remote(&self, remote: &Remote<Verified>) -> Result<Vec<RefString>, VerifyError> {
        // Contains a copy of the signed refs of this remote.
        let mut signed = BTreeMap::from((*remote.refs).clone());
        let mut unsigned = Vec::new();

        // Check all repository references, making sure they are present in the signed refs map.
        for (refname, oid) in self.references_of(&remote.id)? {
            // Skip validation of the signed refs branch, as it is not part of `Remote`.
            if refname == refs::SIGREFS_BRANCH.to_ref_string() {
                continue;
            }
            if let Some(signed_oid) = signed.remove(&refname) {
                if oid != signed_oid {
                    return Err(VerifyError::InvalidRefTarget(remote.id, refname, *oid));
                }
            } else {
                unsigned.push(refname);
            }
        }

        // The refs that are left in the map, are ones that were signed, but are not
        // in the repository. If any are left, bail.
        if let Some((name, _)) = signed.into_iter().next() {
            return Err(VerifyError::MissingRef(remote.id, name));
        }
        // Finally, verify the identity history of remote.
        self.identity_of(&remote.id)?.verified(self.id)?;

        Ok(unsigned)
    }

    fn reference(
        &self,
        remote: &RemoteId,
        name: &git::Qualified,
    ) -> Result<git2::Reference, git::Error> {
        let name = name.with_namespace(remote.into());
        self.backend.find_reference(&name).map_err(git::Error::from)
    }

    fn reference_oid(
        &self,
        remote: &RemoteId,
        reference: &git::Qualified,
    ) -> Result<Oid, git::Error> {
        let name = reference.with_namespace(remote.into());
        let oid = self.backend.refname_to_id(&name)?;

        Ok(oid.into())
    }

    fn commit(&self, oid: Oid) -> Result<git2::Commit, git::Error> {
        self.backend
            .find_commit(oid.into())
            .map_err(git::Error::from)
    }

    fn revwalk(&self, head: Oid) -> Result<git2::Revwalk, git2::Error> {
        let mut revwalk = self.backend.revwalk()?;
        revwalk.push(head.into())?;

        Ok(revwalk)
    }

    fn is_ancestor_of(&self, ancestor: Oid, head: Oid) -> Result<bool, git::Error> {
        self.backend
            .graph_descendant_of(head.into(), ancestor.into())
            .map_err(git::Error::from)
    }

    fn remote(&self, remote: &RemoteId) -> Result<Remote<Verified>, refs::Error> {
        let refs = SignedRefs::load(*remote, self)?;
        Ok(Remote::<Verified>::new(refs))
    }

    fn references_of(&self, remote: &RemoteId) -> Result<Refs, Error> {
        let entries = self
            .backend
            .references_glob(format!("refs/namespaces/{remote}/*").as_str())?;
        let mut refs = BTreeMap::new();

        for e in entries {
            let e = e?;
            let name = e.name().ok_or(Error::InvalidRef)?;
            let (_, refname) = git::parse_ref::<RemoteId>(name)?;
            let oid = e.target().ok_or(Error::InvalidRef)?;
            let (_, category, _, _) = refname.non_empty_components();

            // Only sign known ref categories.
            if [
                git::name::HEADS,
                git::name::TAGS,
                git::name::NOTES,
                &git::name::component!("rad"),
                &git::name::component!("cobs"),
            ]
            .contains(&category.as_ref())
            {
                refs.insert(refname.into(), oid.into());
            }
        }
        Ok(refs.into())
    }

    fn references_glob(
        &self,
        pattern: &self::PatternStr,
    ) -> Result<Vec<(Qualified, Oid)>, self::ext::Error> {
        let mut refs = Vec::new();

        for r in self.backend.references_glob(pattern)? {
            let r = r?;
            let c = r.peel_to_commit()?;

            if let Some(name) = r
                .name()
                .and_then(|n| git::RefStr::try_from_str(n).ok())
                .and_then(git::Qualified::from_refstr)
            {
                refs.push((name.to_owned(), c.id().into()));
            }
        }
        Ok(refs)
    }

    fn remotes(&self) -> Result<Remotes<Verified>, refs::Error> {
        let mut remotes = Vec::new();
        for remote in Repository::remotes(self)? {
            remotes.push(remote?);
        }
        Ok(Remotes::from_iter(remotes))
    }

    fn identity_doc_at(&self, head: Oid) -> Result<identity::Doc<Unverified>, DocError> {
        Doc::<Unverified>::load_at(head, self).map(|(doc, _)| doc)
    }

    fn head(&self) -> Result<(Qualified, Oid), IdentityError> {
        // If `HEAD` is already set locally, just return that.
        if let Ok(head) = self.backend.head() {
            if let Ok((name, oid)) = git::refs::qualified_from(&head) {
                return Ok((name.to_owned(), oid));
            }
        }
        self.canonical_head()
    }

    fn canonical_head(&self) -> Result<(Qualified, Oid), IdentityError> {
        let (_, doc) = self.identity_doc()?;
        let doc = doc.verified()?;
        let project = doc.project()?;
        let branch_ref = Qualified::from(lit::refs_heads(&project.default_branch()));
        let raw = self.raw();

        let mut heads = Vec::new();
        for delegate in doc.delegates.iter() {
            let r = self.reference_oid(delegate, &branch_ref)?.into();

            heads.push(r);
        }

        let oid = match heads.as_slice() {
            [head] => Ok(*head),
            // FIXME: This branch is not tested.
            heads => raw.merge_base_many(heads),
        }?;

        Ok((branch_ref, oid.into()))
    }

    fn identity_head(&self) -> Result<Oid, IdentityError> {
        match Doc::<Verified>::canonical_head(self) {
            Ok(oid) => Ok(oid),
            Err(err) if err.is_not_found() => self.canonical_identity_head(),
            Err(err) => Err(err.into()),
        }
    }

    fn canonical_identity_head(&self) -> Result<Oid, IdentityError> {
        let mut heads = Vec::new();

        for remote in self.remote_ids()? {
            let remote = remote?;
            let oid = Doc::<Unverified>::head(&remote, self)?;

            heads.push(oid.into());
        }
        // Keep track of the longest identity branch.
        let mut longest = heads
            .pop()
            .ok_or_else(|| IdentityError::MissingDelegateIds)?;

        for head in &heads {
            let base = self.raw().merge_base(*head, longest)?;

            if base == longest {
                // `head` is a successor of `longest`. Update `longest`.
                //
                //   o head
                //   |
                //   o longest (base)
                //   |
                //
                longest = *head;
            } else if base == *head || *head == longest {
                // `head` is an ancestor of `longest`, or equal to it. Do nothing.
                //
                //   o longest             o longest, head (base)
                //   |                     |
                //   o head (base)   OR    o
                //   |                     |
                //
            } else {
                // The merge base between `head` and `longest` (`base`)
                // is neither `head` nor `longest`. Therefore, the branches have
                // diverged.
                //
                //    longest   head
                //           \ /
                //            o (base)
                //            |
                //
                return Err(IdentityError::BranchesDiverge);
            }
        }
        Ok(longest.into())
    }

    fn merge_base(&self, left: &Oid, right: &Oid) -> Result<Oid, ext::Error> {
        self.backend
            .merge_base(**left, **right)
            .map(Oid::from)
            .map_err(ext::Error::from)
    }
}

impl WriteRepository for Repository {
    fn set_head(&self) -> Result<Oid, IdentityError> {
        let head_ref = refname!("HEAD");
        let (branch_ref, head) = self.canonical_head()?;

        log::debug!(target: "storage", "Setting ref: {} -> {}", &branch_ref, head);
        self.raw()
            .reference(&branch_ref, *head, true, "set-local-branch (radicle)")?;

        log::debug!(target: "storage", "Setting ref: {} -> {}", head_ref, branch_ref);
        self.raw()
            .reference_symbolic(&head_ref, &branch_ref, true, "set-head (radicle)")?;

        Ok(head)
    }

    fn set_identity_head(&self) -> Result<Oid, IdentityError> {
        let head = self.canonical_identity_head()?;

        log::debug!(target: "storage", "Setting ref: {} -> {}", *CANONICAL_IDENTITY, head);
        self.raw().reference(
            CANONICAL_IDENTITY.as_str(),
            *head,
            true,
            "set-local-branch (radicle)",
        )?;

        Ok(head)
    }

    fn raw(&self) -> &git2::Repository {
        &self.backend
    }
}

impl SignRepository for Repository {
    fn sign_refs<G: Signer>(&self, signer: &G) -> Result<SignedRefs<Verified>, Error> {
        let remote = signer.public_key();
        let refs = self.references_of(remote)?;
        let signed = refs.signed(signer)?;

        signed.save(self)?;

        Ok(signed)
    }
}

impl fetch::Identities for Repository {
    type VerifiedIdentity = Identity<Id>;
    type VerifiedError = IdentityError;

    fn verified(
        &self,
        head: fetch::gix::ObjectId,
    ) -> Result<Self::VerifiedIdentity, Self::VerifiedError> {
        use fetch::gix::oid;

        Identity::load_at(oid::to_oid(head), self)?.verified(self.id)
    }

    fn newer(
        &self,
        left: Self::VerifiedIdentity,
        right: Self::VerifiedIdentity,
    ) -> Result<Self::VerifiedIdentity, fetch::identity::error::History<Self::VerifiedIdentity>>
    {
        use fetch::identity::error::History::Other;
        if self
            .backend
            .graph_descendant_of(left.head.into(), right.current.into())
            .map_err(|e| Other(e.into()))?
        {
            Ok(left)
        } else if self
            .backend
            .graph_descendant_of(right.head.into(), left.current.into())
            .map_err(|e| Other(e.into()))?
        {
            Ok(right)
        } else {
            Err(fetch::identity::error::History::Fork { left, right })
        }
    }
}

impl fetch::sigrefs::Store for Repository {
    type LoadError = refs::Error;

    fn load(
        &self,
        remote: &crypto::PublicKey,
    ) -> Result<Option<fetch::sigrefs::Sigrefs>, Self::LoadError> {
        use fetch::gix::oid;

        let at = match self.reference_oid(remote, &refs::SIGREFS_BRANCH) {
            Ok(at) => at,
            Err(ext::Error::NotFound(_)) => {
                return Ok(None);
            }
            Err(ext::Error::Git(e)) if e.code() == raw::ErrorCode::NotFound => {
                return Ok(None);
            }
            Err(e) => {
                return Err(e.into());
            }
        };
        let refs = SignedRefs::load_at(at, *remote, self)?
            .refs
            .into_iter()
            .map(|(rname, tip)| (rname, oid::to_object_id(tip)))
            .collect();

        Ok(Some(fetch::sigrefs::Sigrefs {
            at: oid::to_object_id(at),
            refs,
        }))
    }

    fn load_at(
        &self,
        tip: impl Into<fetch::gix::ObjectId>,
        remote: &crypto::PublicKey,
    ) -> Result<Option<fetch::sigrefs::Sigrefs>, Self::LoadError> {
        use fetch::gix::oid;

        let tip = tip.into();
        let refs = SignedRefs::load_at(oid::to_oid(tip), *remote, self)?
            .refs
            .into_iter()
            .map(|(rname, tip)| (rname, oid::to_object_id(tip)))
            .collect();

        Ok(Some(fetch::sigrefs::Sigrefs { at: tip, refs }))
    }
}

pub mod trailers {
    use std::str::FromStr;

    use thiserror::Error;

    use super::*;
    use crypto::{PublicKey, PublicKeyError};
    use crypto::{Signature, SignatureError};

    pub const SIGNATURE_TRAILER: &str = "Rad-Signature";

    #[derive(Error, Debug)]
    pub enum Error {
        #[error("invalid format for signature trailer")]
        SignatureTrailerFormat,
        #[error("invalid public key in signature trailer")]
        PublicKey(#[from] PublicKeyError),
        #[error("invalid signature in trailer")]
        Signature(#[from] SignatureError),
    }

    pub fn parse_signatures(msg: &str) -> Result<HashMap<PublicKey, Signature>, Error> {
        let trailers =
            git2::message_trailers_strs(msg).map_err(|_| Error::SignatureTrailerFormat)?;
        let mut signatures = HashMap::with_capacity(trailers.len());

        for (key, val) in trailers.iter() {
            if key == SIGNATURE_TRAILER {
                if let Some((pk, sig)) = val.split_once(' ') {
                    let pk = PublicKey::from_str(pk)?;
                    let sig = Signature::from_str(sig)?;

                    signatures.insert(pk, sig);
                } else {
                    return Err(Error::SignatureTrailerFormat);
                }
            }
        }
        Ok(signatures)
    }
}

pub mod paths {
    use std::path::PathBuf;

    use super::Id;
    use super::ReadStorage;

    pub fn repository<S: ReadStorage>(storage: &S, proj: &Id) -> PathBuf {
        storage.path().join(proj.canonical())
    }
}

#[cfg(test)]
mod tests {
    use crypto::test::signer::MockSigner;

    use super::*;
    use crate::git;
    use crate::storage::refs::SIGREFS_BRANCH;
    use crate::storage::{ReadRepository, ReadStorage};
    use crate::test::arbitrary;
    use crate::test::fixtures;

    #[test]
    fn test_remote_refs() {
        let dir = tempfile::tempdir().unwrap();
        let signer = MockSigner::default();
        let storage = fixtures::storage(dir.path(), &signer).unwrap();
        let inv = storage.inventory().unwrap();
        let proj = inv.first().unwrap();
        let mut refs = git::remote_refs(&git::Url::from(*proj)).unwrap();

        let project = storage.repository(*proj).unwrap();
        let remotes = project.remotes().unwrap();

        // Strip the remote refs of sigrefs so we can compare them.
        for remote in refs.values_mut() {
            let sigref = (*SIGREFS_BRANCH).to_ref_string();
            remote.remove(&sigref).unwrap();
        }

        let remotes = remotes
            .map(|remote| remote.map(|(id, r): (RemoteId, Remote<Verified>)| (id, r.refs.into())))
            .collect::<Result<_, _>>()
            .unwrap();

        assert_eq!(refs, remotes);
    }

    #[test]
    fn test_references_of() {
        let tmp = tempfile::tempdir().unwrap();
        let signer = MockSigner::default();
        let storage = Storage::open(tmp.path().join("storage")).unwrap();

        transport::local::register(storage.clone());

        let (id, _, _, _) =
            fixtures::project(tmp.path().join("project"), &storage, &signer).unwrap();
        let proj = storage.repository(id).unwrap();

        let mut refs = proj
            .references_of(signer.public_key())
            .unwrap()
            .iter()
            .map(|(r, _)| r.to_string())
            .collect::<Vec<_>>();
        refs.sort();

        assert_eq!(
            refs,
            vec!["refs/heads/master", "refs/rad/id", "refs/rad/sigrefs"]
        );
    }

    #[test]
    fn test_sign_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rng = fastrand::Rng::new();
        let signer = MockSigner::new(&mut rng);
        let storage = Storage::open(tmp.path()).unwrap();
        let proj_id = arbitrary::gen::<Id>(1);
        let alice = *signer.public_key();
        let project = storage.create(proj_id).unwrap();
        let backend = &project.backend;
        let sig = git2::Signature::now(&alice.to_string(), "anonymous@radicle.xyz").unwrap();
        let head = git::initial_commit(backend, &sig).unwrap();
        let tree =
            git::write_tree(Path::new("README"), "Hello World!\n".as_bytes(), backend).unwrap();

        git::commit(
            backend,
            &head,
            &git::RefString::try_from(format!("refs/remotes/{alice}/heads/master")).unwrap(),
            "Second commit",
            &sig,
            &tree,
        )
        .unwrap();

        let signed = project.sign_refs(&signer).unwrap();
        let remote = project.remote(&alice).unwrap();
        let mut unsigned = project.references_of(&alice).unwrap();

        // The signed refs doesn't contain the signature ref itself.
        let sigref = (*SIGREFS_BRANCH).to_ref_string();
        unsigned.remove(&sigref).unwrap();

        assert_eq!(remote.refs, signed);
        assert_eq!(*remote.refs, unsigned);
    }
}
