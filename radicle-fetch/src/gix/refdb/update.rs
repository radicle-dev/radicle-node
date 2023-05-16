use either::Either;
use git_ext::ref_format::{Namespaced, RefStr, RefString};
use gix_hash::ObjectId;
use gix_ref::transaction::RefEdit;
use radicle_git_ext::ref_format::Qualified;

/// The set of applied changes from a reference store update.
#[derive(Debug, Default)]
pub struct Applied<'a> {
    /// Set of rejected updates if they did not meet the update
    /// requirements, e.g. concurrent change to previous object id,
    /// broke fast-forward policy, etc.
    pub rejected: Vec<Update<'a>>,
    /// Set of successfully updated references.
    pub updated: Vec<Updated>,
}

impl<'a> Applied<'a> {
    pub fn append(&mut self, other: &mut Self) {
        self.rejected.append(&mut other.rejected);
        self.updated.append(&mut other.updated);
    }
}

#[derive(Clone, Debug)]
pub struct Updates<'a> {
    pub tips: Vec<Update<'a>>,
}

#[derive(Clone, Debug)]
pub enum Update<'a> {
    Direct {
        name: Namespaced<'a>,
        target: ObjectId,

        /// Policy to apply when an [`Update`] would not apply as a
        /// fast-forward.
        no_ff: Policy,
    },
    Symbolic {
        name: Namespaced<'a>,
        target: SymrefTarget<'a>,

        /// Policy to apply when the ref already exists, but is a direct ref
        /// before the update.
        type_change: Policy,
    },
    Prune {
        name: Namespaced<'a>,
        prev: Either<ObjectId, Qualified<'a>>,
    },
}

/// A [`RefEdit`] along with the previous value of the reference.
///
/// If the reference did not exist then `prev` will be the null object
/// id.
#[derive(Clone, Debug)]
pub struct Edit {
    pub edit: RefEdit,
    pub prev: ObjectId,
}

impl<'a> Update<'a> {
    pub fn refname(&self) -> &Namespaced<'a> {
        match self {
            Update::Direct { name, .. } => name,
            Update::Symbolic { name, .. } => name,
            Update::Prune { name, .. } => name,
        }
    }

    pub fn into_owned<'b>(self) -> Update<'b> {
        match self {
            Self::Direct {
                name,
                target,
                no_ff,
            } => Update::Direct {
                name: name.into_owned(),
                target,
                no_ff,
            },
            Self::Symbolic {
                name,
                target,
                type_change,
            } => Update::Symbolic {
                name: name.into_owned(),
                target: target.into_owned(),
                type_change,
            },
            Self::Prune { name, prev } => Update::Prune {
                name: name.into_owned(),
                prev: prev.map_right(|q| q.into_owned()),
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Policy {
    /// Abort the entire transaction.
    Abort,
    /// Reject this update, but continue the transaction.
    Reject,
    /// Allow the update.
    Allow,
}

#[derive(Clone, Debug)]
pub struct SymrefTarget<'a> {
    pub name: Namespaced<'a>,
    pub target: ObjectId,
}

impl<'a> SymrefTarget<'a> {
    pub fn name(&self) -> &RefStr {
        self.name.as_ref()
    }

    pub fn into_owned<'b>(self) -> SymrefTarget<'b> {
        SymrefTarget {
            name: self.name.to_owned(),
            target: self.target,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Updated {
    Direct {
        name: RefString,
        prev: ObjectId,
        target: ObjectId,
    },
    Symbolic {
        name: RefString,
        prev: ObjectId,
        target: RefString,
    },
    Prune {
        name: RefString,
        prev: ObjectId,
    },
}
