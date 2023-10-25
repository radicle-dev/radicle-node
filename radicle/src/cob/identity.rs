use std::collections::BTreeMap;
use std::{fmt, ops::Deref, str::FromStr};

use crypto::{PublicKey, Signature};
use once_cell::sync::Lazy;
use radicle_cob::{ObjectId, TypeName};
use radicle_crypto::{Signer, Verified};
use radicle_git_ext as git_ext;
use radicle_git_ext::Oid;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    cob,
    cob::{
        op, store,
        store::{Cob, CobAction, Transaction},
        ActorId, Timestamp,
    },
    identity::{
        doc::{Doc, DocError, Id},
        Did,
    },
    storage::{ReadRepository, RepositoryError, WriteRepository},
};

use super::{Author, EntryId};

/// Type name of an identity proposal.
pub static TYPENAME: Lazy<TypeName> =
    Lazy::new(|| FromStr::from_str("xyz.radicle.id").expect("type name is valid"));

/// Identity operation.
pub type Op = cob::Op<Action>;

/// Identifier for an identity revision.
pub type RevisionId = EntryId;

/// Proposal operation.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    #[serde(rename = "revision")]
    Revision {
        /// Short summary of changes.
        title: String,
        /// Longer comment on proposed changes.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        description: String,
        /// Blob identifier of the document included in this action as an embed.
        /// Hence, we do not include it as a parent of this action in [`CobAction`].
        blob: Oid,
        /// Parent revision that this revision replaces.
        parent: Option<RevisionId>,
        /// Signature over the revision blob.
        signature: Signature,
    },
    RevisionEdit {
        /// The revision to edit.
        revision: RevisionId,
        /// Short summary of changes.
        title: String,
        /// Longer comment on proposed changes.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        description: String,
    },
    #[serde(rename = "revision.accept")]
    RevisionAccept {
        revision: RevisionId,
        /// Signature over the blob.
        signature: Signature,
    },
    #[serde(rename = "revision.reject")]
    RevisionReject { revision: RevisionId },
    #[serde(rename = "revision.redact")]
    RevisionRedact { revision: RevisionId },
}

impl CobAction for Action {}

/// Error applying an operation onto a state.
#[derive(Error, Debug)]
pub enum ApplyError {
    /// Causal dependency missing.
    ///
    /// This error indicates that the operations are not being applied
    /// in causal order, which is a requirement for this CRDT.
    ///
    /// For example, this can occur if an operation references anothern operation
    /// that hasn't happened yet.
    #[error("causal dependency {0:?} missing")]
    Missing(EntryId),
    /// General error initializing an identity.
    #[error("initialization failed: {0}")]
    Init(&'static str),
    /// Invalid signature over document blob.
    #[error("invalid signature from {0} for blob {1}")]
    InvalidSignature(PublicKey, Oid),
    /// Unauthorized action.
    #[error("not authorized to perform this action")]
    NotAuthorized,
    #[error("parent id is missing from revision")]
    MissingParent,
    #[error("verdict for this revision has already been applied")]
    DuplicateVerdict,
    #[error("revision is in an unexpected state")]
    UnexpectedState,
    #[error("revision has been redacted")]
    Redacted,
    #[error("document does not contain any changes to current identity")]
    DocUnchanged,
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("git: {0}")]
    GitExt(#[from] git_ext::Error),
    #[error("identity document error: {0}")]
    Doc(#[from] DocError),
}

/// Error updating or creating proposals.
#[derive(Error, Debug)]
pub enum Error {
    #[error("apply failed: {0}")]
    Apply(#[from] ApplyError),
    #[error("store: {0}")]
    Store(#[from] store::Error),
    #[error("op decoding failed: {0}")]
    Op(#[from] op::OpEncodingError),
    #[error(transparent)]
    Doc(#[from] DocError),
}

/// An evolving identity document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The canonical identifier for this identity.
    /// This is the object id of the initial document blob.
    pub id: Id,
    /// The current revision of the document.
    /// Equal to the head of the identity branch.
    pub current: RevisionId,
    /// The initial revision of the document.
    pub root: RevisionId,
    /// The latest revision that each delegate has accepted.
    /// Delegates can only accept one revision at a time.
    pub heads: BTreeMap<Did, RevisionId>,

    /// Revisions.
    revisions: BTreeMap<RevisionId, Option<Revision>>,
    /// Timeline of events.
    timeline: Vec<EntryId>,
}

impl std::ops::Deref for Identity {
    type Target = Revision;

    fn deref(&self) -> &Self::Target {
        self.current()
    }
}

impl Identity {
    pub fn new(revision: Revision) -> Self {
        let root = revision.id;

        Self {
            id: revision.blob.into(),
            root,
            current: root,
            heads: revision
                .delegates
                .iter()
                .copied()
                .map(|did| (did, root))
                .collect(),
            revisions: BTreeMap::from_iter([(root, Some(revision))]),
            timeline: vec![root],
        }
    }

    pub fn initialize<'a, R: WriteRepository + cob::Store, G: Signer>(
        doc: &Doc<Verified>,
        store: &'a R,
        signer: &G,
    ) -> Result<IdentityMut<'a, R>, cob::store::Error> {
        let mut store = cob::store::Store::open(store)?;
        let (id, identity) =
            Transaction::<Identity, _>::initial("Initialize identity", &mut store, signer, |tx| {
                tx.revision("Initial revision", "", doc, None, signer)
            })?;

        Ok(IdentityMut {
            id,
            identity,
            store,
        })
    }

    pub fn get<R: ReadRepository + cob::Store>(
        object: &ObjectId,
        repo: &R,
    ) -> Result<Identity, store::Error> {
        cob::get::<Self, _>(repo, Self::type_name(), object)
            .map(|r| r.map(|cob| cob.object))?
            .ok_or_else(move || store::Error::NotFound(TYPENAME.clone(), *object))
    }

    /// Get a proposal mutably.
    pub fn get_mut<'a, R: WriteRepository + cob::Store>(
        id: &ObjectId,
        repo: &'a R,
    ) -> Result<IdentityMut<'a, R>, store::Error> {
        let obj = Self::get(id, repo)?;
        let store = cob::store::Store::open(repo)?;

        Ok(IdentityMut {
            id: *id,
            identity: obj,
            store,
        })
    }

    pub fn load<R: ReadRepository + cob::Store>(repo: &R) -> Result<Identity, RepositoryError> {
        let oid = repo.identity_root()?;
        let oid = ObjectId::from(oid);

        Self::get(&oid, repo).map_err(RepositoryError::from)
    }

    pub fn load_mut<R: WriteRepository + cob::Store>(
        repo: &R,
    ) -> Result<IdentityMut<R>, RepositoryError> {
        let oid = repo.identity_root()?;
        let oid = ObjectId::from(oid);

        Self::get_mut(&oid, repo).map_err(RepositoryError::from)
    }
}

impl Identity {
    /// The repository identifier.
    pub fn id(&self) -> Id {
        self.id
    }

    /// The current document.
    pub fn doc(&self) -> &Doc<Verified> {
        &self.current().doc
    }

    /// The current revision.
    pub fn current(&self) -> &Revision {
        self.revision(&self.current)
            .expect("Identity::current: the current revision must always exist")
    }

    /// The initial revision of this identity.
    pub fn root(&self) -> &Revision {
        self.revision(&self.root)
            .expect("Identity::root: the root revision must always exist")
    }

    /// The head of the identity branch. This points to a commit that
    /// contains the current document blob.
    pub fn head(&self) -> Oid {
        self.current
    }

    /// A specific [`Revision`], that may be redacted.
    pub fn revision(&self, revision: &RevisionId) -> Option<&Revision> {
        self.revisions.get(revision).and_then(|r| r.as_ref())
    }

    /// All the [`Revision`]s that have not been redacted.
    pub fn revisions(&self) -> impl DoubleEndedIterator<Item = &Revision> {
        self.timeline
            .iter()
            .filter_map(|id| self.revisions.get(id).and_then(|o| o.as_ref()))
    }

    pub fn latest_by(&self, who: &Did) -> Option<&Revision> {
        self.revisions().rev().find(|r| r.author.id() == who)
    }
}

impl store::Cob for Identity {
    type Action = Action;
    type Error = ApplyError;

    fn type_name() -> &'static TypeName {
        &TYPENAME
    }

    fn from_root<R: ReadRepository>(op: Op, repo: &R) -> Result<Self, Self::Error> {
        let mut actions = op.actions.into_iter();
        let Some(
            Action::Revision { title, description, blob, signature, parent }
        ) = actions.next() else {
            return Err(ApplyError::Init("the first action must be of type `revision`"));
        };
        if parent.is_some() {
            return Err(ApplyError::Init(
                "the initial revision must not have a parent",
            ));
        }
        if actions.next().is_some() {
            return Err(ApplyError::Init(
                "the first operation must contain only one action",
            ));
        }
        let root = Doc::<Verified>::load_at(op.id, repo)?;
        if root.blob != blob {
            return Err(ApplyError::Init("invalid object id specified in revision"));
        }
        if root.blob != *repo.id() {
            return Err(ApplyError::Init(
                "repository root does not match identifier",
            ));
        }
        assert_eq!(root.commit, op.id);

        let founder = root.delegates.first();
        if founder.as_key() != &op.author {
            return Err(ApplyError::Init("delegate does not match committer"));
        }
        // Verify signature against root document. Since there is no previous document,
        // we verify it against itself.
        if root
            .verify_signature(founder, &signature, root.blob)
            .is_err()
        {
            return Err(ApplyError::InvalidSignature(**founder, root.blob));
        }
        let revision = Revision::new(
            root.commit,
            title,
            description,
            op.author.into(),
            root.blob,
            root.doc,
            State::Accepted,
            signature,
            parent,
            op.timestamp,
        );
        Ok(Identity::new(revision))
    }

    fn op<R: ReadRepository>(&mut self, op: Op, repo: &R) -> Result<(), ApplyError> {
        let id = op.id;

        for action in op.actions {
            match self.action(action, id, op.author, op.timestamp, repo) {
                Ok(()) => {}
                // This particular error is returned when there is a mismatch between the expected
                // and the actual state of a revision, which can happen concurrently. Therefore
                // it is not fatal and we simply ignore it.
                Err(ApplyError::UnexpectedState) => {}
                // It's not a user error if the revision happens to be redacted by
                // the time this action is processed.
                Err(ApplyError::Redacted) => {}
                Err(other) => return Err(other),
            }
            debug_assert!(!self.timeline.contains(&id));
            self.timeline.push(id);
        }
        Ok(())
    }
}

impl Identity {
    /// Apply a single action to the identity document.
    ///
    /// This function ensures a few things:
    /// * Only delegates can interact with the state.
    /// * There is only ever one accepted revision; this is the "current" revision.
    /// * There can be zero or more active revisions, up to the number of delegates.
    /// * An active revision is one that can be "voted" on.
    /// * An active revision always has the current revision as parent.
    /// * Only the active revision can be accepted, rejected or edited.
    fn action<R: ReadRepository>(
        &mut self,
        action: Action,
        entry: EntryId,
        author: ActorId,
        timestamp: Timestamp,
        repo: &R,
    ) -> Result<(), ApplyError> {
        let current = self.current().clone();

        if !current.is_delegate(&author) {
            return Err(ApplyError::UnexpectedState);
        }
        match action {
            Action::RevisionAccept {
                revision,
                signature,
            } => {
                let id = revision;
                let Some(revision) = lookup::revision_mut(&mut self.revisions, &id)? else {
                    return Err(ApplyError::Redacted);
                };
                if !revision.is_active() {
                    // You can't vote on an inactive revision.
                    return Err(ApplyError::UnexpectedState);
                }
                assert_eq!(revision.parent, Some(current.id));

                self.heads.insert(author.into(), id);
                revision.accept(author, signature, &current)?;

                self.adopt(id);
            }
            Action::RevisionReject { revision } => {
                let Some(revision) = lookup::revision_mut(&mut self.revisions, &revision)? else {
                    return Err(ApplyError::Redacted);
                };
                if !revision.is_active() {
                    // You can't vote on an inactive revision.
                    return Err(ApplyError::UnexpectedState);
                }
                assert_eq!(revision.parent, Some(current.id));

                revision.reject(author)?;
            }
            Action::RevisionEdit {
                title,
                description,
                revision,
            } => {
                if revision == self.current {
                    return Err(ApplyError::NotAuthorized);
                }
                let Some(revision) = lookup::revision_mut(&mut self.revisions, &revision)? else {
                    return Err(ApplyError::Redacted);
                };
                if !revision.is_active() {
                    // You can't edit an inactive revision.
                    return Err(ApplyError::UnexpectedState);
                }
                if revision.author.public_key() != &author {
                    // Can't edit someone else's revision.
                    // Since the author never changes, we can safely mark this as invalid.
                    return Err(ApplyError::NotAuthorized);
                }
                assert_eq!(revision.parent, Some(current.id));

                revision.title = title;
                revision.description = description;
            }
            Action::RevisionRedact { revision } => {
                if revision == self.current {
                    // Can't redact the current revision.
                    return Err(ApplyError::UnexpectedState);
                }
                if let Some(revision) = self.revisions.get_mut(&revision) {
                    if let Some(r) = revision {
                        if r.is_accepted() {
                            // You can't redact an accepted revision.
                            return Err(ApplyError::UnexpectedState);
                        }
                        if r.author.public_key() != &author {
                            // Can't redact someone else's revision.
                            // Since the author never changes, we can safely mark this as invalid.
                            return Err(ApplyError::NotAuthorized);
                        }
                        *revision = None;
                    }
                } else {
                    return Err(ApplyError::Missing(revision));
                }
            }
            Action::Revision {
                title,
                description,
                blob,
                signature,
                parent,
            } => {
                debug_assert!(!self.revisions.contains_key(&entry));

                let doc = repo.blob(blob)?;
                let doc = Doc::from_blob(&doc)?;
                // All revisions but the first one must have a parent.
                let Some(parent) = parent else {
                    return Err(ApplyError::MissingParent);
                };
                let Some(parent) = lookup::revision(&self.revisions, &parent)? else {
                    return Err(ApplyError::Redacted);
                };
                // If the parent of this revision is no longer the current document, this
                // revision can be marked as outdated.
                let state = if parent.id == current.id {
                    // If the revision is not outdated, we expect it to make a change to the
                    // current version.
                    if doc == parent.doc {
                        return Err(ApplyError::DocUnchanged);
                    }
                    State::Active
                } else {
                    State::Stale
                };

                // Verify signature over new blob, using trusted delegates.
                if parent.verify_signature(&author, &signature, blob).is_err() {
                    return Err(ApplyError::InvalidSignature(author, blob));
                }
                let revision = Revision::new(
                    entry,
                    title,
                    description,
                    author.into(),
                    blob,
                    doc,
                    state,
                    signature,
                    Some(parent.id),
                    timestamp,
                );
                let id = revision.id;

                self.heads.insert(author.into(), id);
                self.revisions.insert(id, Some(revision));

                if state == State::Active {
                    self.adopt(id);
                }
            }
        }
        Ok(())
    }

    /// Try to adopt a revision as the current one.
    fn adopt(&mut self, id: RevisionId) {
        if self.current == id {
            return;
        }
        let votes = self
            .heads
            .values()
            .filter(|revision| **revision == id)
            .count();
        if self.is_majority(votes) {
            self.current = id;
            self.current_mut().state = State::Accepted;

            // Void all other active revisions.
            for r in self
                .revisions
                .iter_mut()
                .filter_map(|(_, r)| r.as_mut())
                .filter(|r| r.state == State::Active)
            {
                r.state = State::Stale;
            }
        }
    }

    /// A specific [`Revision`], mutably.
    fn revision_mut(&mut self, revision: &RevisionId) -> Option<&mut Revision> {
        self.revisions.get_mut(revision).and_then(|r| r.as_mut())
    }

    /// The current revision, mutably.
    fn current_mut(&mut self) -> &mut Revision {
        let current = self.current;
        self.revision_mut(&current)
            .expect("Identity::current_mut: the current revision must always exist")
    }
}

impl<R: ReadRepository> cob::Evaluate<R> for Identity {
    type Error = Error;

    fn init(entry: &cob::Entry, repo: &R) -> Result<Self, Self::Error> {
        let op = Op::try_from(entry)?;
        let object = Identity::from_root(op, repo)?;

        Ok(object)
    }

    fn apply(&mut self, entry: &cob::Entry, repo: &R) -> Result<(), Self::Error> {
        let op = Op::try_from(entry)?;

        self.op(op, repo).map_err(Error::Apply)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// An accepting verdict must supply the [`Signature`] over the
    /// new proposed [`Doc`].
    Accept(Signature),
    /// Rejecting the proposed [`Doc`].
    Reject,
}

/// State of a revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// The revision is actively being voted on. From here, it can go into any of the
    /// other states.
    Active,
    /// The revision has been accepted by a majority of delegates. Once accepted,
    /// a revision doesn't change state.
    Accepted,
    /// The revision was rejected by a majority of delegates. Once rejected,
    /// a revision doesn't change state.
    Rejected,
    /// The revision was active, but has been replaced by another revision,
    /// and is now outdated. Once stale, a revision doesn't change state.
    Stale,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Accepted => write!(f, "accepted"),
            Self::Rejected => write!(f, "rejected"),
            Self::Stale => write!(f, "stale"),
        }
    }
}

/// A new [`Doc`] for an [`Identity`]. The revision can be
/// reviewed by gathering [`Signature`]s for accepting the changes, or
/// rejecting them.
///
/// Once a revision has reached the quorum threshold of the previous
/// [`Identity`] it is then adopted as the current identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    /// The id of this revision. Points to a commit.
    pub id: RevisionId,
    /// Identity document blob at this revision.
    pub blob: Oid,
    /// Title of the proposal.
    pub title: String,
    /// State of the revision.
    pub state: State,
    /// Description of the proposal.
    pub description: String,
    /// Author of this proposed revision.
    pub author: Author,
    /// New [`Doc`] that will replace `previous`' document.
    pub doc: Doc<Verified>,
    /// Physical timestamp of this proposal revision.
    pub timestamp: Timestamp,
    /// Parent revision.
    pub parent: Option<RevisionId>,

    /// Signatures and rejections given by the delegates.
    verdicts: BTreeMap<PublicKey, Verdict>,
}

impl std::ops::Deref for Revision {
    type Target = Doc<Verified>;

    fn deref(&self) -> &Self::Target {
        &self.doc
    }
}

impl Revision {
    pub fn signatures(&self) -> impl Iterator<Item = (&PublicKey, Signature)> {
        self.verdicts().filter_map(|(key, verdict)| match verdict {
            Verdict::Accept(sig) => Some((key, *sig)),
            Verdict::Reject => None,
        })
    }

    pub fn is_accepted(&self) -> bool {
        matches!(self.state, State::Accepted)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Active)
    }

    pub fn verdicts(&self) -> impl Iterator<Item = (&PublicKey, &Verdict)> {
        self.verdicts.iter()
    }

    pub fn accepted(&self) -> impl Iterator<Item = Did> + '_ {
        self.signatures().map(|(key, _)| key.into())
    }

    pub fn rejected(&self) -> impl Iterator<Item = Did> + '_ {
        self.verdicts().filter_map(|(key, v)| match v {
            Verdict::Accept(_) => None,
            Verdict::Reject => Some(key.into()),
        })
    }

    pub fn sign<G: Signer>(&self, signer: &G) -> Result<Signature, DocError> {
        self.doc.signature_of(signer)
    }
}

// Private functions that may not do all the verification. Use with caution.
impl Revision {
    fn new(
        id: RevisionId,
        title: String,
        description: String,
        author: Author,
        blob: Oid,
        doc: Doc<Verified>,
        state: State,
        signature: Signature,
        parent: Option<RevisionId>,
        timestamp: Timestamp,
    ) -> Self {
        let verdicts = BTreeMap::from_iter([(*author.public_key(), Verdict::Accept(signature))]);

        Self {
            id,
            title,
            description,
            author,
            blob,
            doc,
            state,
            verdicts,
            parent,
            timestamp,
        }
    }

    fn accept(
        &mut self,
        author: PublicKey,
        signature: Signature,
        current: &Revision,
    ) -> Result<(), ApplyError> {
        // Check that this is a valid signature over the new document blob id.
        if current
            .verify_signature(&author, &signature, self.blob)
            .is_err()
        {
            return Err(ApplyError::InvalidSignature(author, self.blob));
        }
        if self
            .verdicts
            .insert(author, Verdict::Accept(signature))
            .is_some()
        {
            return Err(ApplyError::DuplicateVerdict);
        }
        Ok(())
    }

    fn reject(&mut self, key: PublicKey) -> Result<(), ApplyError> {
        if self.verdicts.insert(key, Verdict::Reject).is_some() {
            return Err(ApplyError::DuplicateVerdict);
        }
        // Mark as rejected if it's impossible for this revision to be accepted
        // with the current delegate set. Note that if the delegate set changes,
        // this proposal will be marked as `stale` anyway.
        if self.is_active() && self.rejected().count() > self.delegates.len() - self.majority() {
            self.state = State::Rejected;
        }
        Ok(())
    }
}

impl<R: ReadRepository> store::Transaction<Identity, R> {
    pub fn accept(
        &mut self,
        revision: RevisionId,
        signature: Signature,
    ) -> Result<(), store::Error> {
        self.push(Action::RevisionAccept {
            revision,
            signature,
        })
    }

    pub fn reject(&mut self, revision: RevisionId) -> Result<(), store::Error> {
        self.push(Action::RevisionReject { revision })
    }

    pub fn edit(
        &mut self,
        revision: RevisionId,
        title: impl ToString,
        description: impl ToString,
    ) -> Result<(), store::Error> {
        self.push(Action::RevisionEdit {
            revision,
            title: title.to_string(),
            description: description.to_string(),
        })
    }

    pub fn redact(&mut self, revision: RevisionId) -> Result<(), store::Error> {
        self.push(Action::RevisionRedact { revision })
    }

    pub fn revision<G: Signer>(
        &mut self,
        title: impl ToString,
        description: impl ToString,
        doc: &Doc<Verified>,
        parent: Option<RevisionId>,
        signer: &G,
    ) -> Result<(), store::Error> {
        let (blob, content, signature) = doc.sign(signer).map_err(store::Error::Identity)?;

        // Identity document.
        self.embed([cob::Embed {
            name: String::from("radicle.json"),
            content,
        }])?;

        // Revision metadata.
        self.push(Action::Revision {
            title: title.to_string(),
            description: description.to_string(),
            blob,
            parent,
            signature,
        })
    }
}

pub struct IdentityMut<'a, R> {
    pub id: ObjectId,

    identity: Identity,
    store: store::Store<'a, Identity, R>,
}

impl<'a, R> fmt::Debug for IdentityMut<'a, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityMut")
            .field("id", &self.id)
            .field("identity", &self.identity)
            .finish()
    }
}

impl<'a, R> IdentityMut<'a, R>
where
    R: WriteRepository + cob::Store,
{
    /// Reload the identity data from storage.
    pub fn reload(&mut self) -> Result<(), store::Error> {
        self.identity = self
            .store
            .get(&self.id)?
            .ok_or_else(|| store::Error::NotFound(TYPENAME.clone(), self.id))?;

        Ok(())
    }

    pub fn transaction<G, F>(
        &mut self,
        message: &str,
        signer: &G,
        operations: F,
    ) -> Result<EntryId, Error>
    where
        G: Signer,
        F: FnOnce(&mut Transaction<Identity, R>) -> Result<(), store::Error>,
    {
        let mut tx = Transaction::default();
        operations(&mut tx)?;

        let (doc, commit) = tx.commit(message, self.id, &mut self.store, signer)?;
        self.identity = doc;

        Ok(commit)
    }

    /// Update the identity by proposing a new revision.
    /// If the signer is the only delegate, the revision is accepted automatically.
    pub fn update<G: Signer>(
        &mut self,
        title: impl ToString,
        description: impl ToString,
        doc: &Doc<Verified>,
        signer: &G,
    ) -> Result<Revision, Error> {
        let parent = self.current;
        let id = self.transaction("Propose revision", signer, |tx| {
            tx.revision(title, description, doc, Some(parent), signer)
        })?;

        // SAFETY: Since the revision was just added, it's guaranteed to be there.
        Ok(self
            .revision(&id)
            .expect("IdentityMut::update: revision exists")
            .clone())
    }

    /// Accept an active revision.
    pub fn accept<G: Signer>(&mut self, revision: &Revision, signer: &G) -> Result<EntryId, Error> {
        let signature = revision.sign(signer)?;

        self.transaction("Accept revision", signer, |tx| {
            tx.accept(revision.id, signature)
        })
    }

    /// Reject an active revision.
    pub fn reject<G: Signer>(
        &mut self,
        revision: RevisionId,
        signer: &G,
    ) -> Result<EntryId, Error> {
        self.transaction("Reject revision", signer, |tx| tx.reject(revision))
    }

    /// Redact a revision.
    pub fn redact<G: Signer>(
        &mut self,
        revision: RevisionId,
        signer: &G,
    ) -> Result<EntryId, Error> {
        self.transaction("Redact revision", signer, |tx| tx.redact(revision))
    }

    /// Edit an active revision's title or description.
    pub fn edit<G: Signer>(
        &mut self,
        revision: RevisionId,
        title: String,
        description: String,
        signer: &G,
    ) -> Result<EntryId, Error> {
        self.transaction("Edit revision", signer, |tx| {
            tx.edit(revision, title, description)
        })
    }
}

impl<'a, R> Deref for IdentityMut<'a, R> {
    type Target = Identity;

    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}

mod lookup {
    use super::*;

    pub fn revision_mut<'a>(
        revisions: &'a mut BTreeMap<RevisionId, Option<Revision>>,
        revision: &RevisionId,
    ) -> Result<Option<&'a mut Revision>, ApplyError> {
        match revisions.get_mut(revision) {
            Some(Some(revision)) => Ok(Some(revision)),
            // Redacted.
            Some(None) => Ok(None),
            // Missing. Causal error.
            None => Err(ApplyError::Missing(*revision)),
        }
    }

    pub fn revision<'a>(
        revisions: &'a BTreeMap<RevisionId, Option<Revision>>,
        revision: &RevisionId,
    ) -> Result<Option<&'a Revision>, ApplyError> {
        match revisions.get(revision) {
            Some(Some(revision)) => Ok(Some(revision)),
            // Redacted.
            Some(None) => Ok(None),
            // Missing. Causal error.
            None => Err(ApplyError::Missing(*revision)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod test {
    use qcheck_macros::quickcheck;
    use radicle_crypto::test::signer::MockSigner;
    use radicle_crypto::Signer as _;

    use crate::crypto::PublicKey;
    use crate::identity::Visibility;
    use crate::rad;
    use crate::storage::git::Storage;
    use crate::storage::ReadStorage;
    use crate::test::fixtures;
    use crate::test::setup::{Network, NodeWithRepo};

    use super::*;
    use crate::identity::did::Did;
    use crate::identity::doc::PayloadId;

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
    fn test_identity_updates() {
        let NodeWithRepo { node, repo } = NodeWithRepo::default();
        let bob = MockSigner::default();
        let signer = &node.signer;
        let mut identity = Identity::load_mut(&*repo).unwrap();
        let mut doc = identity.doc().clone();
        let title = "Identity update";
        let description = "";
        let r0 = identity.current;

        // The initial state is accepted.
        assert!(identity.current().is_accepted());
        // Using an identical document to the current one fails.
        identity
            .update(title, description, &doc, signer)
            .unwrap_err();
        assert_eq!(identity.current, r0);

        // Change threshold to `2`, even though there's only one delegate. This should
        // fail as it makes the master branch immutable.
        doc.threshold = 2;
        identity
            .update(title, description, &doc, signer)
            .unwrap_err();
        assert_eq!(identity.current, r0);
        // Let's add another delegate.
        doc.delegate(bob.public_key());
        // The update should go through now.
        let r1 = identity
            .update(title, description, &doc, signer)
            .unwrap()
            .id;
        assert!(identity.revision(&r1).unwrap().is_accepted());
        assert_eq!(identity.current, r1);
        // With two delegates now, we need two signatures for any update to go through.
        // So this next update shouldn't be accepted as canonical until the second delegate
        // signs it.
        doc.visibility = Visibility::private([]);
        let r2 = identity.update(title, description, &doc, signer).unwrap();
        // R1 is still the head.
        assert_eq!(identity.current, r1);
        assert_eq!(r2.state, State::Active);
        assert_eq!(repo.canonical_identity_head().unwrap(), r1);
        assert_eq!(repo.identity_doc().unwrap().visibility, Visibility::Public);
        // Now let's add a signature on R2 from Bob.
        identity.accept(&r2, &bob).unwrap();

        // R2 is now the head.
        assert_eq!(identity.current, r2.id);
        assert_eq!(identity.revision(&r2.id).unwrap().state, State::Accepted);
        assert_eq!(repo.canonical_identity_head().unwrap(), r2.id);
        assert_eq!(
            repo.canonical_identity_doc().unwrap().visibility,
            Visibility::private([])
        );
    }

    #[test]
    fn test_identity_update_rejected() {
        let NodeWithRepo { node, repo } = NodeWithRepo::default();
        let bob = MockSigner::default();
        let eve = MockSigner::default();
        let signer = &node.signer;
        let mut identity = Identity::load_mut(&*repo).unwrap();
        let mut doc = identity.doc().clone();
        let title = "Identity update";
        let description = "";

        // Let's add another delegate.
        doc.delegate(bob.public_key());
        let r1 = identity
            .update(title, description, &doc, signer)
            .unwrap()
            .id;
        assert_eq!(identity.current, r1);

        doc.visibility = Visibility::private([]);
        let r2 = identity
            .update("Make private", description, &doc, &node.signer)
            .unwrap();

        // 1/2 rejected means that we can never reach the required 2/2 votes.
        identity.reject(r2.id, &bob).unwrap();
        let r2 = identity.revision(&r2.id).unwrap();
        assert_eq!(r2.state, State::Rejected);

        // Now let's add another delegate.
        doc.delegate(eve.public_key());
        let r3 = identity
            .update("Add Eve", description, &doc, &node.signer)
            .unwrap();
        let _ = identity.accept(&r3, &bob).unwrap();
        assert_eq!(identity.current, r3.id);

        doc.visibility = Visibility::Public;
        let r3 = identity
            .update("Make public", description, &doc, &node.signer)
            .unwrap();

        // 1/3 rejected means that we can still reach the 2/3 required votes.
        identity.reject(r3.id, &bob).unwrap();
        let r3 = identity.revision(&r3.id).unwrap().clone();
        assert_eq!(r3.state, State::Active); // Still active.

        // 2/3 rejected means that we can no longer reach the 2/3 required votes.
        identity.reject(r3.id, &eve).unwrap();
        let r3 = identity.revision(&r3.id).unwrap();
        assert_eq!(r3.state, State::Rejected);
    }

    #[test]
    #[ignore]
    // Run with `RAD_COMMIT_TIME=1514817556`.
    fn test_identity_updates_concurrent() {
        let network = Network::default();
        let alice = &network.alice;
        let bob = &network.bob;

        let mut alice_identity = Identity::load_mut(&*alice.repo).unwrap();
        let mut alice_doc = alice_identity.doc().clone();

        alice_doc.delegate(bob.signer.public_key());
        let a1 = alice_identity
            .update("Add Bob", "", &alice_doc, &alice.signer)
            .unwrap()
            .id;

        bob.repo.fetch(alice);

        let mut bob_identity = Identity::load_mut(&*bob.repo).unwrap();
        let bob_doc = bob_identity.doc().clone();
        assert!(bob_doc.is_delegate(bob.signer.public_key()));

        // Alice changes the document without making Bob aware.
        alice_doc.visibility = Visibility::private([]);
        let a2 = alice_identity
            .update("Change visibility", "", &alice_doc, &alice.signer)
            .unwrap();
        // Bob makes the same change without knowing Alice already did.
        let b1 = bob_identity
            .update("Make private", "", &alice_doc, &bob.signer)
            .unwrap()
            .id;

        // Bob gets Alice's data.
        bob.repo.fetch(alice);
        bob_identity.reload().unwrap();
        assert_eq!(bob_identity.current, a1);

        // Alice gets Bob's data.
        // There's not enough votes for either of these proposals to pass.
        alice.repo.fetch(bob);
        alice_identity.reload().unwrap();
        assert_eq!(alice_identity.current, a1);
        assert_eq!(bob_identity.revision(&a2.id).unwrap().state, State::Active);
        assert_eq!(bob_identity.revision(&b1).unwrap().state, State::Active);

        // Now Bob accepts Alice's proposal. This voids his own.
        bob_identity.accept(&a2, &bob.signer).unwrap();
        assert_eq!(bob_identity.current, a2.id);
        assert_eq!(bob_identity.revision(&a1).unwrap().state, State::Accepted);
        assert_eq!(
            bob_identity.revision(&a2.id).unwrap().state,
            State::Accepted
        );
        assert_eq!(bob_identity.revision(&b1).unwrap().state, State::Stale);
    }

    #[test]
    #[ignore]
    // Run with `RAD_COMMIT_TIME=1514817556`.
    fn test_identity_redact_revision() {
        let network = Network::default();
        let alice = &network.alice;
        let bob = &network.bob;
        let eve = &network.eve;

        let mut alice_identity = Identity::load_mut(&*alice.repo).unwrap();
        let mut alice_doc = alice_identity.doc().clone();

        alice_doc.delegate(bob.signer.public_key());
        let a0 = alice_identity.root;
        let a1 = alice_identity
            .update("Add Bob", "Eh.", &alice_doc, &alice.signer)
            .unwrap()
            .id;

        alice_doc.visibility = Visibility::private([eve.signer.public_key().into()]);
        let a2 = alice_identity
            .update("Change visibility", "Eh.", &alice_doc, &alice.signer)
            .unwrap();

        bob.repo.fetch(alice);
        let a3 = alice_identity.redact(a2.id, &alice.signer).unwrap();
        assert!(alice_identity.revision(&a1).is_some());
        assert_eq!(alice_identity.timeline, vec![a0, a1, a2.id, a3]);

        let mut bob_identity = Identity::load_mut(&*bob.repo).unwrap();
        let b1 = bob_identity.accept(&a2, &bob.signer).unwrap();

        assert_eq!(bob_identity.timeline, vec![a0, a1, a2.id, b1]);
        assert_eq!(
            bob_identity.revision(&a2.id).unwrap().state,
            State::Accepted
        );
        bob.repo.fetch(alice);
        bob_identity.reload().unwrap();

        assert_eq!(bob_identity.timeline, vec![a0, a1, a2.id, a3, b1]);
        assert_eq!(bob_identity.revision(&a2.id), None);
        assert_eq!(bob_identity.current, a1);
    }

    #[test]
    #[ignore]
    // Run with `RAD_COMMIT_TIME=1514817556`.
    fn test_identity_remove_delegate_concurrent() {
        let network = Network::default();
        let alice = &network.alice;
        let bob = &network.bob;
        let eve = &network.eve;

        let mut alice_identity = Identity::load_mut(&*alice.repo).unwrap();
        let mut alice_doc = alice_identity.doc().clone();

        alice_doc.delegate(bob.signer.public_key());
        alice_doc.delegate(eve.signer.public_key());
        let a0 = alice_identity.root;
        let a1 = alice_identity
            .update("Add Bob and Eve", "Eh.", &alice_doc, &alice.signer)
            .unwrap()
            .id;

        alice_doc.rescind(eve.signer.public_key()).unwrap();
        let a2 = alice_identity
            .update("Remove Eve", "", &alice_doc, &alice.signer)
            .unwrap();

        bob.repo.fetch(eve);
        bob.repo.fetch(alice);
        eve.repo.fetch(bob);

        let mut bob_identity = Identity::load_mut(&*bob.repo).unwrap();
        let b1 = bob_identity.accept(&a2, &bob.signer).unwrap();
        assert_eq!(bob_identity.current, a2.id);

        let mut eve_identity = Identity::load_mut(&*eve.repo).unwrap();
        let mut eve_doc = eve_identity.doc().clone();
        eve_doc.visibility = Visibility::private([eve.signer.public_key().into()]);
        let e1 = eve_identity
            .update("Change visibility", "", &eve_doc, &eve.signer)
            .unwrap();

        eve.repo.fetch(bob);
        eve_identity.reload().unwrap();
        assert_eq!(eve_identity.timeline, vec![a0, a1, a2.id, e1.id, b1]);
        assert!(!eve_identity.is_delegate(eve.signer.public_key()));
    }

    #[test]
    #[ignore]
    // Run with `RAD_COMMIT_TIME=1514817556`.
    fn test_identity_reject_concurrent() {
        let network = Network::default();
        let alice = &network.alice;
        let bob = &network.bob;
        let eve = &network.eve;

        let mut alice_identity = Identity::load_mut(&*alice.repo).unwrap();
        let mut alice_doc = alice_identity.doc().clone();

        alice_doc.delegate(bob.signer.public_key());
        alice_doc.delegate(eve.signer.public_key());
        let a0 = alice_identity.root;
        let a1 = alice_identity
            .update("Add Bob and Eve", "Eh.", &alice_doc, &alice.signer)
            .unwrap()
            .id;

        alice_doc.visibility = Visibility::private([]);
        let a2 = alice_identity
            .update("Change visibility", "", &alice_doc, &alice.signer)
            .unwrap();

        bob.repo.fetch(eve);
        bob.repo.fetch(alice);
        eve.repo.fetch(bob);

        // Bob accepts alice's revision.
        let mut bob_identity = Identity::load_mut(&*bob.repo).unwrap();
        let b1 = bob_identity.accept(&a2, &bob.signer).unwrap();

        // Eve rejects the revision, not knowing.
        let mut eve_identity = Identity::load_mut(&*eve.repo).unwrap();
        let e1 = eve_identity.reject(a2.id, &eve.signer).unwrap();

        // Then she submits a new revision.
        let mut eve_doc = eve_identity.doc().clone();
        eve_doc.visibility = Visibility::private([eve.signer.public_key().into()]);
        let e2 = eve_identity
            .update("Change visibility", "", &eve_doc, &eve.signer)
            .unwrap();

        // Though the rules are that you cannot reject an already accepted revision,
        // since this update was done concurrently there was no way of knowing. Therefore,
        // an error shouldn't be returned. We simply ignore the rejection.

        eve.repo.fetch(bob);
        eve_identity.reload().unwrap();
        assert_eq!(eve_identity.timeline, vec![a0, a1, a2.id, e1, b1, e2.id]);

        // Her revision is there, although stale, since another revision was accepted since.
        // However, it wasn't pruned, even though rejecting an accepted revision is an error.
        let e2 = eve_identity.revision(&e2.id).unwrap();
        assert_eq!(e2.state, State::Stale);
    }

    #[test]
    #[ignore]
    // Run with `RAD_COMMIT_TIME=1514817556`.
    fn test_identity_updates_concurrent_outdated() {
        let network = Network::default();
        let alice = &network.alice;
        let bob = &network.bob;
        let eve = &network.eve;

        let mut alice_identity = Identity::load_mut(&*alice.repo).unwrap();
        let mut alice_doc = alice_identity.doc().clone();

        alice.repo.fetch(bob);
        alice.repo.fetch(eve);
        alice_doc.delegate(bob.signer.public_key());
        alice_doc.delegate(eve.signer.public_key());
        let a0 = alice_identity.root;
        let a1 = alice_identity
            .update("Add Bob and Eve", "", &alice_doc, &alice.signer)
            .unwrap();

        bob.repo.fetch(alice);
        eve.repo.fetch(alice);

        let mut bob_identity = Identity::load_mut(&*bob.repo).unwrap();
        let mut bob_doc = bob_identity.doc().clone();
        assert!(bob_doc.is_delegate(bob.signer.public_key()));

        //  a2 e1
        //  | /
        //  b1
        //  |
        //  a1
        //  |
        //  a0

        // Bob and Alice change the document visibility. Eve is not aware.
        bob_doc.visibility = Visibility::private([]);
        let b1 = bob_identity
            .update("Change visibility #1", "", &bob_doc, &bob.signer)
            .unwrap();
        alice.repo.fetch(bob);
        eve.repo.fetch(bob);

        // In the meantime, Eve does the same thing on her side.
        let mut eve_identity = Identity::load_mut(&*eve.repo).unwrap();
        let mut eve_doc = eve_identity.doc().clone();
        eve_doc.visibility = Visibility::private([]);
        let e1 = eve_identity
            .update("Change visibility #2", "Woops", &eve_doc, &eve.signer)
            .unwrap();
        assert_eq!(eve_identity.revisions().count(), 4);
        assert_eq!(e1.state, State::Active);

        let a2 = alice_identity.accept(&b1, &alice.signer).unwrap();

        eve.repo.fetch(alice);
        eve_identity.reload().unwrap();

        assert_eq!(eve_identity.timeline, vec![a0, a1.id, b1.id, e1.id, a2]);
        assert_eq!(eve_identity.revision(&e1.id).unwrap().state, State::Stale);
    }

    #[test]
    fn test_valid_identity() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut rng = fastrand::Rng::new();

        let alice = MockSigner::new(&mut rng);
        let bob = MockSigner::new(&mut rng);
        let eve = MockSigner::new(&mut rng);

        let storage = Storage::open(tempdir.path().join("storage"), fixtures::user()).unwrap();
        let (id, _, _, _) =
            fixtures::project(tempdir.path().join("copy"), &storage, &alice).unwrap();

        // Bob and Eve fork the project from Alice.
        rad::fork_remote(id, alice.public_key(), &bob, &storage).unwrap();
        rad::fork_remote(id, alice.public_key(), &eve, &storage).unwrap();

        let repo = storage.repository(id).unwrap();
        let mut identity = Identity::load_mut(&repo).unwrap();
        let mut doc = identity.doc().clone();
        let prj = doc.project().unwrap();

        // Make a change to the description and sign it.
        let desc = prj.description().to_owned() + "!";
        let prj = prj.update(None, desc, None).unwrap();
        doc.payload.insert(PayloadId::project(), prj.clone().into());
        identity
            .update("Update description", "", &doc, &alice)
            .unwrap();

        // Add Bob as a delegate, and sign it.
        doc.delegate(bob.public_key());
        doc.threshold = 2;
        identity.update("Add bob", "", &doc, &alice).unwrap();

        // Add Eve as a delegate.
        doc.delegate(eve.public_key());

        // Update with both Bob and Alice's signature.
        let revision = identity.update("Add eve", "", &doc, &alice).unwrap();
        identity.accept(&revision, &bob).unwrap();

        // Update description again with signatures by Eve and Bob.
        let desc = prj.description().to_owned() + "?";
        let prj = prj.update(None, desc, None).unwrap();
        doc.payload.insert(PayloadId::project(), prj.into());

        let revision = identity
            .update("Update description again", "Bob's repository", &doc, &bob)
            .unwrap();
        identity.accept(&revision, &eve).unwrap();

        let identity: Identity = Identity::load(&repo).unwrap();
        let root = repo.identity_root().unwrap();
        let doc = repo.identity_doc_at(revision.id).unwrap();

        assert_eq!(identity.signatures().count(), 2);
        assert_eq!(identity.revisions().count(), 5);
        assert_eq!(identity.id(), id);
        assert_eq!(identity.root().id, root);
        assert_eq!(identity.current().blob, doc.blob);
        assert_eq!(identity.current().description.as_str(), "Bob's repository");
        assert_eq!(identity.head(), revision.id);
        assert_eq!(identity.doc(), &*doc);
        assert_eq!(
            identity.doc().project().unwrap().description(),
            "Acme's repository!?"
        );

        assert_eq!(doc.project().unwrap().description(), "Acme's repository!?");
    }
}
