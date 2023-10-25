#![warn(clippy::unwrap_used)]
pub mod cob;
pub mod transport;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::{fs, io};

use crypto::{Signer, Verified};
use once_cell::sync::Lazy;

use crate::crypto::Unverified;
use crate::git;
use crate::identity::doc::DocError;
use crate::identity::{doc::DocAt, Doc, Id};
use crate::identity::{Identity, Project};
use crate::storage::refs;
use crate::storage::refs::{Refs, SignedRefs};
use crate::storage::{
    Inventory, ReadRepository, ReadStorage, Remote, Remotes, RepositoryError, SignRepository,
    WriteRepository, WriteStorage,
};

pub use crate::git::*;
pub use crate::storage::Error;

use super::{RemoteId, RemoteRepository, ValidateRepository};

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

/// Basic repository information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInfo<V> {
    /// Repository identifier.
    pub rid: Id,
    /// Head of default branch.
    pub head: Oid,
    /// Identity document.
    pub doc: Doc<V>,
}

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
        let oid = r.resolve()?.target().ok_or(RefError::NoTarget)?;

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
    info: UserInfo,
}

impl ReadStorage for Storage {
    type Repository = Repository;

    fn info(&self) -> &UserInfo {
        &self.info
    }

    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn path_of(&self, rid: &Id) -> PathBuf {
        paths::repository(&self, rid)
    }

    fn contains(&self, rid: &Id) -> Result<bool, RepositoryError> {
        if paths::repository(&self, rid).exists() {
            let _ = self.repository(*rid)?.head()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn inventory(&self) -> Result<Inventory, Error> {
        let repos = self.repositories()?;

        Ok(repos
            .into_iter()
            .filter(|r| r.doc.visibility.is_public())
            .map(|r| r.rid)
            .collect())
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
        Repository::create(paths::repository(self, &rid), rid, &self.info)
    }

    fn remove(&self, rid: Id) -> Result<(), Error> {
        self.repository(rid)?.remove()
    }
}

impl Storage {
    // TODO: Return a better error when not found.
    pub fn open<P: AsRef<Path>>(path: P, info: UserInfo) -> Result<Self, io::Error> {
        let path = path.as_ref().to_path_buf();

        match fs::create_dir_all(&path) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
            Ok(()) => {}
        }

        Ok(Self { path, info })
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn repositories(&self) -> Result<Vec<RepositoryInfo<Verified>>, Error> {
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

            let repo = match self.repository(rid) {
                Ok(repo) => repo,
                Err(e) => {
                    log::warn!(target: "storage", "Repository {rid} is invalid: {e}");
                    continue;
                }
            };
            let doc = match repo.identity_doc() {
                Ok(doc) => doc.into(),
                Err(e) => {
                    log::warn!(target: "storage", "Repository {rid} is invalid: looking up doc: {e}");
                    continue;
                }
            };

            // For performance reasons, we don't do a full repository check here.
            let head = match repo.head() {
                Ok((_, head)) => head,
                Err(e) => {
                    log::warn!(target: "storage", "Repository {rid} is invalid: looking up head: {e}");
                    continue;
                }
            };
            repos.push(RepositoryInfo { rid, head, doc });
        }
        Ok(repos)
    }

    pub fn inspect(&self) -> Result<(), Error> {
        for r in self.repositories()? {
            let rid = r.rid;
            let repo = self.repository(rid)?;

            for r in repo.raw().references()? {
                let r = r?;
                let name = r.name().ok_or(Error::InvalidRef)?;
                let oid = r.resolve()?.target().ok_or(Error::InvalidRef)?;

                println!("{} {oid} {name}", rid.urn());
            }
        }
        Ok(())
    }
}

/// Git implementation of [`WriteRepository`] using the `git2` crate.
pub struct Repository {
    /// The repository identifier (RID).
    pub id: Id,
    /// The backing Git repository.
    pub backend: git2::Repository,
}

/// A set of [`Validation`] errors that a caller **must use**.
#[must_use]
#[derive(Debug, Default)]
pub struct Validations(pub Vec<Validation>);

impl Validations {
    pub fn append(&mut self, vs: &mut Self) {
        self.0.append(&mut vs.0)
    }
}

impl IntoIterator for Validations {
    type Item = Validation;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl Deref for Validations {
    type Target = Vec<Validation>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Validations {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Validation errors that can occur when verifying the layout of the
/// storage. These errors include checking the validity of the
/// `rad/sigrefs` contents and the identity of the repository.
#[derive(Debug, Error)]
pub enum Validation {
    #[error("found unsigned ref `{0}`")]
    UnsignedRef(RefString),
    #[error("{refname}: expected {expected}, but found {actual}")]
    MismatchedRef {
        expected: Oid,
        actual: Oid,
        refname: RefString,
    },
    #[error("missing `refs/namespaces/{remote}/{refname}`")]
    MissingRef {
        remote: RemoteId,
        refname: RefString,
    },
    #[error("missing `refs/namespaces/{0}/refs/rad/sigrefs`")]
    MissingRadSigRefs(RemoteId),
}

impl Repository {
    /// Open an existing repository.
    pub fn open<P: AsRef<Path>>(path: P, id: Id) -> Result<Self, Error> {
        let backend = git2::Repository::open_bare(path.as_ref())?;

        Ok(Self { id, backend })
    }

    /// Create a new repository.
    pub fn create<P: AsRef<Path>>(path: P, id: Id, info: &UserInfo) -> Result<Self, Error> {
        let backend = git2::Repository::init_opts(
            &path,
            git2::RepositoryInitOptions::new()
                .bare(true)
                .no_reinit(true)
                .external_template(false),
        )?;
        let mut config = backend.config()?;

        config.set_str("user.name", &info.name())?;
        config.set_str("user.email", &info.email())?;

        Ok(Self { id, backend })
    }

    /// Remove an existing repository
    pub fn remove(&self) -> Result<(), Error> {
        let path = self.backend.path();
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    /// Create the repository's identity branch.
    pub fn init<G: Signer, S: WriteStorage>(
        doc: &Doc<Verified>,
        storage: S,
        signer: &G,
    ) -> Result<(Self, git::Oid), RepositoryError> {
        let (doc_oid, _) = doc.encode()?;
        let id = Id::from(doc_oid);
        let repo = Self::create(paths::repository(&storage, &id), id, storage.info())?;
        let commit = doc.init(&repo, signer)?;

        Ok((repo, commit))
    }

    pub fn inspect(&self) -> Result<(), Error> {
        for r in self.backend.references()? {
            let r = r?;
            let name = r.name().ok_or(Error::InvalidRef)?;
            let oid = r.resolve()?.target().ok_or(Error::InvalidRef)?;

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
                    Err(err) => Err(err.into()),
                    Ok(r) => Ok(Some(r)),
                }
            })
            .filter_map(Result::transpose);

        Ok(refs)
    }

    /// Get the canonical project information.
    pub fn project(&self) -> Result<Project, RepositoryError> {
        let head = self.identity_head()?;
        let doc = self.identity_doc_at(head)?;
        let proj = doc.project()?;

        Ok(proj)
    }

    pub fn identity_doc_of(&self, remote: &RemoteId) -> Result<Doc<Verified>, DocError> {
        let oid = self.identity_head_of(remote)?;
        Doc::load_at(oid, self).map(|d| d.into())
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

impl RemoteRepository for Repository {
    fn remotes(&self) -> Result<Remotes<Verified>, refs::Error> {
        let mut remotes = Vec::new();
        for remote in Repository::remotes(self)? {
            remotes.push(remote?);
        }
        Ok(Remotes::from_iter(remotes))
    }

    fn remote(&self, remote: &RemoteId) -> Result<Remote<Verified>, refs::Error> {
        let refs = SignedRefs::load(*remote, self)?;
        Ok(Remote::<Verified>::new(refs))
    }
}

impl ValidateRepository for Repository {
    fn validate_remote(&self, remote: &Remote<Verified>) -> Result<Validations, Error> {
        // Contains a copy of the signed refs of this remote.
        let mut signed = BTreeMap::from((*remote.refs).clone());
        let mut failures = Validations::default();
        let mut has_sigrefs = false;

        // Check all repository references, making sure they are present in the signed refs map.
        for (refname, oid) in self.references_of(&remote.id)? {
            // Skip validation of the signed refs branch, as it is not part of `Remote`.
            if refname == refs::SIGREFS_BRANCH.to_ref_string() {
                has_sigrefs = true;
                continue;
            }
            if let Some(signed_oid) = signed.remove(&refname) {
                if oid != signed_oid {
                    failures.push(Validation::MismatchedRef {
                        refname,
                        expected: signed_oid,
                        actual: oid,
                    });
                }
            } else {
                failures.push(Validation::UnsignedRef(refname));
            }
        }

        if !has_sigrefs {
            failures.push(Validation::MissingRadSigRefs(remote.id));
        }

        // The refs that are left in the map, are ones that were signed, but are not
        // in the repository. If any are left, bail.
        if let Some((name, _)) = signed.into_iter().next() {
            failures.push(Validation::MissingRef {
                refname: name,
                remote: remote.id,
            });
        }

        // Nb. As it stands, it doesn't make sense to verify a single remote's identity branch,
        // since it is a COB.

        Ok(failures)
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

    fn blob_at<P: AsRef<Path>>(&self, commit: Oid, path: P) -> Result<git2::Blob, git::Error> {
        let commit = self.backend.find_commit(*commit)?;
        let tree = commit.tree()?;
        let entry = tree.get_path(path.as_ref())?;
        let obj = entry.to_object(&self.backend)?;
        let blob = obj.into_blob().map_err(|_| {
            git::Error::NotFound(git::NotFound::NoSuchBlob(
                path.as_ref().display().to_string(),
            ))
        })?;

        Ok(blob)
    }

    fn blob(&self, oid: Oid) -> Result<git2::Blob, git::Error> {
        self.backend.find_blob(oid.into()).map_err(git::Error::from)
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

    fn references_of(&self, remote: &RemoteId) -> Result<Refs, Error> {
        let entries = self
            .backend
            .references_glob(format!("refs/namespaces/{remote}/*").as_str())?;
        let mut refs = BTreeMap::new();

        for e in entries {
            let e = e?;
            let name = e.name().ok_or(Error::InvalidRef)?;
            let (_, refname) = git::parse_ref::<RemoteId>(name)?;
            let oid = e.resolve()?.target().ok_or(Error::InvalidRef)?;
            let (_, category, _, _) = refname.non_empty_components();

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

    fn identity_doc_at(&self, head: Oid) -> Result<DocAt, DocError> {
        Doc::<Verified>::load_at(head, self)
    }

    fn head(&self) -> Result<(Qualified, Oid), RepositoryError> {
        // If `HEAD` is already set locally, just return that.
        if let Ok(head) = self.backend.head() {
            if let Ok((name, oid)) = git::refs::qualified_from(&head) {
                return Ok((name.to_owned(), oid));
            }
        }
        self.canonical_head()
    }

    fn canonical_head(&self) -> Result<(Qualified, Oid), RepositoryError> {
        let doc = self.identity_doc()?;
        let project = doc.project()?;
        let branch_ref = git::refs::branch(project.default_branch());
        let raw = self.raw();
        let mut heads = Vec::new();

        for delegate in doc.delegates.iter() {
            let r = self.reference_oid(delegate, &branch_ref)?;

            heads.push(*r);
        }
        let quorum = self::quorum(&heads, doc.threshold, raw)?;

        Ok((branch_ref, quorum))
    }

    fn identity_head(&self) -> Result<Oid, RepositoryError> {
        let result = self
            .backend
            .refname_to_id(CANONICAL_IDENTITY.as_str())
            .map(Oid::from);

        match result {
            Ok(oid) => Ok(oid),
            Err(err) if git::ext::is_not_found_err(&err) => self.canonical_identity_head(),
            Err(err) => Err(err.into()),
        }
    }

    fn identity_head_of(&self, remote: &RemoteId) -> Result<Oid, git::ext::Error> {
        self.reference_oid(remote, &git::refs::storage::IDENTITY_BRANCH)
    }

    fn identity_root(&self) -> Result<Oid, RepositoryError> {
        let oid = self.backend.refname_to_id(CANONICAL_IDENTITY.as_str())?;
        let walk = self.revwalk(oid.into())?.collect::<Vec<_>>();
        let root = walk
            .into_iter()
            .last()
            .ok_or(RepositoryError::Doc(DocError::Missing))??;

        Ok(root.into())
    }

    fn identity_root_of(&self, remote: &RemoteId) -> Result<Oid, RepositoryError> {
        let oid = self.identity_head_of(remote)?;
        let walk = self.revwalk(oid)?.collect::<Vec<_>>();
        let root = walk
            .into_iter()
            .last()
            .ok_or(RepositoryError::Doc(DocError::Missing))??;

        Ok(root.into())
    }

    fn canonical_identity_head(&self) -> Result<Oid, RepositoryError> {
        for remote in self.remote_ids()? {
            let remote = remote?;
            // Nb. A remote may not have an identity document if the user has not contributed
            // any changes to the identity COB.
            let Ok(root) = self.identity_root_of(&remote) else {
                continue;
            };
            let blob = Doc::<Unverified>::blob_at(root, self)?;

            // We've got an identity that goes back to the correct root.
            if blob.id() == **self.id {
                let identity = Identity::get(&root.into(), self)?;

                return Ok(identity.head());
            }
        }
        Err(DocError::Missing.into())
    }

    fn merge_base(&self, left: &Oid, right: &Oid) -> Result<Oid, ext::Error> {
        self.backend
            .merge_base(**left, **right)
            .map(Oid::from)
            .map_err(ext::Error::from)
    }
}

impl WriteRepository for Repository {
    fn set_head(&self) -> Result<Oid, RepositoryError> {
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

    fn set_identity_head_to(&self, commit: Oid) -> Result<(), RepositoryError> {
        log::debug!(target: "storage", "Setting ref: {} -> {}", *CANONICAL_IDENTITY, commit);
        self.raw().reference(
            CANONICAL_IDENTITY.as_str(),
            *commit,
            true,
            "set-local-branch (radicle)",
        )?;
        Ok(())
    }

    fn set_user(&self, info: &UserInfo) -> Result<(), Error> {
        let mut config = self.backend.config()?;
        config.set_str("user.name", &info.name())?;
        config.set_str("user.email", &info.email())?;
        Ok(())
    }

    fn raw(&self) -> &git2::Repository {
        &self.backend
    }
}

impl SignRepository for Repository {
    fn sign_refs<G: Signer>(&self, signer: &G) -> Result<SignedRefs<Verified>, Error> {
        let remote = signer.public_key();
        let mut refs = self.references_of(remote)?;
        // Don't sign the `rad/sigrefs` ref itself, and don't sign invalid OIDs.
        refs.retain(|name, oid| {
            name.as_refstr() != refs::SIGREFS_BRANCH.as_ref() && !oid.is_zero()
        });
        let signed = refs.signed(signer)?;

        signed.save(self)?;

        Ok(signed)
    }
}

#[derive(Debug, Error)]
pub enum QuorumError {
    #[error("no quorum was found")]
    NoQuorum,
    #[error(transparent)]
    Git(#[from] git2::Error),
}

/// Computes the quorum or "canonical" head based on the given heads and the
/// threshold. This can be described as the latest commit that is included in
/// at least `threshold` histories. In case there are multiple heads passing
/// the threshold, and they are divergent, their merge base is taken.
///
/// Returns an error if `heads` is empty or `threshold` cannot be satisified with
/// the number of heads given.
pub fn quorum(
    heads: &[git::raw::Oid],
    threshold: usize,
    repo: &git::raw::Repository,
) -> Result<Oid, QuorumError> {
    let mut direct: HashMap<git::raw::Oid, HashSet<usize>> = HashMap::new();
    let mut indirect: HashMap<git::raw::Oid, HashSet<usize>> = HashMap::new();

    let Some(init) = heads.first() else {
        return Err(QuorumError::NoQuorum);
    };
    // Nb. The merge base chosen for two merge commits is arbitrary.
    let base = heads
        .iter()
        .try_fold(*init, |base, h| repo.merge_base(base, *h))?;

    // Score every commit in the graph with the number of heads
    // pointing to it.
    // To make sure the votes are not counted twice, we use
    // the index in the `heads` slice as the vote identifier.
    // Note that it's perfectly legal to have multiple heads
    // with the same value.
    for (i, head) in heads.iter().enumerate() {
        direct.entry(*head).or_default().insert(i);

        let mut revwalk = repo.revwalk()?;
        revwalk.push(*head)?;

        for rev in revwalk {
            let rev = rev?;
            indirect.entry(rev).or_default().insert(i);

            if rev == base {
                break;
            }
        }
    }

    {
        let matches = direct
            .iter()
            .filter(|(_, tips)| tips.len() >= threshold)
            .map(|(h, _)| *h)
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [] => {
                // Check indirect votes.
            }
            [head] => return Ok((*head).into()),
            [head, ref rest @ ..] => {
                let oid = rest
                    .iter()
                    .try_fold(*head, |base, h| repo.merge_base(base, *h))?;

                if !direct.contains_key(&oid) {
                    return Ok(oid.into());
                }
            }
        }
    }

    let mut combined: HashMap<git::raw::Oid, HashSet<usize>> = HashMap::new();
    for (k, v) in direct.into_iter().chain(indirect) {
        combined.entry(k).or_default().extend(v);
    }

    let minimum = combined
        .iter()
        .filter(|(_, tips)| tips.len() >= threshold)
        .map(|(_, tips)| tips.len())
        .min()
        .ok_or(QuorumError::NoQuorum)?;

    let candidates = combined
        .iter()
        .filter(|(_, v)| v.len() == minimum)
        .map(|(h, _)| *h)
        .collect::<Vec<_>>();

    let oid = match candidates.as_slice() {
        [] => return Err(QuorumError::NoQuorum),
        [head] => *head,
        [head, ref rest @ ..] => rest
            .iter()
            .try_fold(*head, |base, h| repo.merge_base(base, *h))?,
    };

    Ok(oid.into())
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
#[allow(clippy::unwrap_used)]
mod tests {
    use crypto::test::signer::MockSigner;

    use super::*;
    use crate::assert_matches;
    use crate::git;
    use crate::storage::refs::SIGREFS_BRANCH;
    use crate::storage::{ReadRepository, ReadStorage};
    use crate::test::arbitrary;
    use crate::test::fixtures;

    #[test]
    fn test_quorum_properties() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, c0) = fixtures::repository(tmp.path());
        let c0: git::Oid = c0.into();
        let a1 = fixtures::commit("A1", &[*c0], &repo);
        let a2 = fixtures::commit("A2", &[*a1], &repo);
        let d1 = fixtures::commit("D1", &[*c0], &repo);
        let c1 = fixtures::commit("C1", &[*c0], &repo);
        let c2 = fixtures::commit("C2", &[*c1], &repo);
        let b2 = fixtures::commit("B2", &[*c1], &repo);
        let a1 = fixtures::commit("A1", &[*c0], &repo);
        let m1 = fixtures::commit("M1", &[*c2, *b2], &repo);
        let m2 = fixtures::commit("M2", &[*a1, *b2], &repo);
        let mut rng = fastrand::Rng::new();
        let choices = vec![*c0, *c1, *c2, *b2, *a1, *a2, *d1, *m1, *m2];

        for _ in 0..100 {
            let count = rng.usize(1..=choices.len());
            let threshold = rng.usize(1..=count);
            let mut heads = Vec::new();

            for _ in 0..count {
                let ix = rng.usize(0..choices.len());
                heads.push(choices[ix]);
            }
            rng.shuffle(&mut heads);

            match quorum(&heads, threshold, &repo) {
                Ok(canonical) => {
                    let mut matches = 0;
                    for h in &heads {
                        if *canonical == *h || repo.graph_descendant_of(*h, *canonical).unwrap() {
                            matches += 1;
                        }
                    }
                    assert!(
                        matches >= threshold,
                        "test failed: heads={heads:?} threshold={threshold} canonical={canonical}"
                    );
                }
                Err(e) => panic!("{e} for heads={heads:?} threshold={threshold}"),
            }
        }
    }

    #[test]
    fn test_quorum() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, c0) = fixtures::repository(tmp.path());
        let c0: git::Oid = c0.into();
        let c1 = fixtures::commit("C1", &[*c0], &repo);
        let c2 = fixtures::commit("C2", &[*c1], &repo);
        let b2 = fixtures::commit("B2", &[*c1], &repo);
        let a1 = fixtures::commit("A1", &[*c0], &repo);
        let m1 = fixtures::commit("M1", &[*c2, *b2], &repo);
        let m2 = fixtures::commit("M2", &[*a1, *b2], &repo);

        eprintln!("C0: {c0}");
        eprintln!("C1: {c1}");
        eprintln!("C2: {c2}");
        eprintln!("B2: {b2}");
        eprintln!("A1: {a1}");
        eprintln!("M1: {m1}");
        eprintln!("M2: {m2}");

        assert_eq!(quorum(&[*c0], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*c0], 0, &repo).unwrap(), c0);
        assert_matches!(quorum(&[], 0, &repo), Err(QuorumError::NoQuorum));
        assert_matches!(quorum(&[*c0], 2, &repo), Err(QuorumError::NoQuorum));

        //  C1
        //  |
        // C0
        assert_eq!(quorum(&[*c1], 1, &repo).unwrap(), c1);

        //   C2
        //   |
        //  C1
        //  |
        // C0
        assert_eq!(quorum(&[*c1, *c2], 1, &repo).unwrap(), c2);
        assert_eq!(quorum(&[*c1, *c2], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c0, *c1, *c2], 3, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*c1, *c1, *c2], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c1, *c1, *c2], 1, &repo).unwrap(), c2);
        assert_eq!(quorum(&[*c2, *c2, *c1], 1, &repo).unwrap(), c2);

        // B2 C2
        //   \|
        //   C1
        //   |
        //  C0
        assert_eq!(quorum(&[*c1, *c2, *b2], 1, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c2, *b2], 1, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*b2, *c2], 1, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c2, *b2], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*b2, *c2], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c1, *c2, *b2], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c1, *c2, *b2], 3, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*b2, *b2, *c2], 2, &repo).unwrap(), b2);
        assert_eq!(quorum(&[*b2, *c2, *c2], 2, &repo).unwrap(), c2);
        assert_eq!(quorum(&[*b2, *b2, *c2, *c2], 1, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*b2, *c2, *c2], 1, &repo).unwrap(), c1);

        //  B2 C2
        //    \|
        // A1 C1
        //   \|
        //   C0
        assert_eq!(quorum(&[*c2, *b2, *a1], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*c2, *b2, *a1], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c2, *b2, *a1], 3, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*c1, *c2, *b2, *a1], 4, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*c0, *c1, *c2, *b2, *a1], 2, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c0, *c1, *c2, *b2, *a1], 3, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*c0, *c2, *b2, *a1], 3, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*c0, *c1, *c2, *b2, *a1], 4, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*a1, *a1, *c2, *c2, *c1], 2, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*a1, *a1, *c2, *c2, *c1], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*a1, *a1, *c2], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*b2, *b2, *c2, *c2], 1, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*b2, *b2, *c2, *c2, *a1], 1, &repo).unwrap(), c0);

        //    M2  M1
        //    /\  /\
        //    \ B2 C2
        //     \  \|
        //     A1 C1
        //       \|
        //       C0
        assert_eq!(quorum(&[*m1], 1, &repo).unwrap(), m1);
        assert_eq!(quorum(&[*m1, *m2], 1, &repo).unwrap(), b2);
        assert_eq!(quorum(&[*m2, *m1], 1, &repo).unwrap(), b2);
        assert_eq!(quorum(&[*m1, *m2], 2, &repo).unwrap(), b2);
        assert_eq!(quorum(&[*m1, *m2, *c2], 1, &repo).unwrap(), c1);
        assert_eq!(quorum(&[*m1, *a1], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*m1, *a1], 2, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*m1, *m1, *b2], 2, &repo).unwrap(), m1);
        assert_eq!(quorum(&[*c2, *m1, *m2], 3, &repo).unwrap(), c1);
    }

    #[test]
    #[ignore = "failing"]
    fn test_quorum_merges() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, c0) = fixtures::repository(tmp.path());
        let c0: git::Oid = c0.into();
        let c1 = fixtures::commit("C1", &[*c0], &repo);
        let c2 = fixtures::commit("C2", &[*c0], &repo);
        let c3 = fixtures::commit("C3", &[*c0], &repo);

        let m1 = fixtures::commit("M1", &[*c1, *c2], &repo);
        let m2 = fixtures::commit("M2", &[*c2, *c3], &repo);

        eprintln!("C0: {c0}");
        eprintln!("C1: {c1}");
        eprintln!("C2: {c2}");
        eprintln!("C3: {c3}");
        eprintln!("M1: {m1}");
        eprintln!("M2: {m2}");

        assert_eq!(quorum(&[*m1, *m2], 1, &repo).unwrap(), c2);
        assert_eq!(quorum(&[*m1, *m2], 2, &repo).unwrap(), c2);

        let m3 = fixtures::commit("M3", &[*c2, *c1], &repo);

        assert_eq!(quorum(&[*m1, *m3], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*m1, *m3], 2, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*m3, *m1], 1, &repo).unwrap(), c0);
        assert_eq!(quorum(&[*m3, *m1], 2, &repo).unwrap(), c0);
    }

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
        let storage = Storage::open(tmp.path().join("storage"), fixtures::user()).unwrap();

        transport::local::register(storage.clone());

        let (rid, _, _, _) =
            fixtures::project(tmp.path().join("project"), &storage, &signer).unwrap();
        let repo = storage.repository(rid).unwrap();
        let id = repo.identity().unwrap().head();
        let cob = format!("refs/cobs/xyz.radicle.id/{id}");

        let mut refs = repo
            .references_of(signer.public_key())
            .unwrap()
            .iter()
            .map(|(r, _)| r.to_string())
            .collect::<Vec<_>>();
        refs.sort();

        assert_eq!(
            refs,
            vec![&cob, "refs/heads/master", "refs/rad/id", "refs/rad/sigrefs"]
        );
    }

    #[test]
    fn test_sign_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rng = fastrand::Rng::new();
        let signer = MockSigner::new(&mut rng);
        let storage = Storage::open(tmp.path(), fixtures::user()).unwrap();
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
