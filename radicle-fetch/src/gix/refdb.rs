mod internal;

mod mem;
pub use mem::InMemory;

pub mod error;

mod update;
pub use update::{Applied, Edit, Policy, SymrefTarget, Update, Updated, Updates};

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use bstr::{BString, ByteVec};
use either::{Either, Either::*};
use gix_actor::{date::Time, Signature};
use gix_hash::ObjectId;
use gix_protocol::handshake;
use gix_ref::{
    file::iter::LooseThenPacked,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
    FullName, Reference, Target,
};
use radicle_crypto::PublicKey;
use radicle_git_ext::ref_format::{Namespaced, Qualified, RefString};

use super::{odb, Odb};

/// The user information used for signing commits and configuring the
/// `name` and `email` fields in the Git config.
#[derive(Debug, Clone)]
pub struct UserInfo {
    /// Alias of the local peer.
    pub alias: String,
    /// [`PublicKey`] of the local peer.
    pub key: PublicKey,
}

impl UserInfo {
    /// The name of the user, i.e. the `alias`.
    pub fn name(&self) -> String {
        self.alias.clone()
    }

    /// The "email" of the user, which is in the form
    /// `<alias>@<public key>`.
    pub fn email(&self) -> String {
        format!("{}@{}", self.alias, self.key)
    }

    /// The [`Signature`] of the user, using the local time at the
    /// time of invocation.
    pub fn signature(&self) -> Signature {
        Signature {
            name: BString::from(self.alias.as_str()),
            email: format!("{}@{}", self.alias, self.key).into(),
            time: Time::now_local_or_utc(),
        }
    }
}

/// A reference in the [`Refdb`].
pub struct Ref {
    /// The name the reference can be found under.
    pub name: Qualified<'static>,
    /// Whether the target of the reference is direct, i.e. point to
    /// an [`ObjectId`], or symbolic, i.e. points to another reference
    /// name.
    pub target: Either<ObjectId, Qualified<'static>>,
    /// The target of the reference if all symbolic links were
    /// followed, if any.
    pub peeled: ObjectId,
}

impl TryFrom<handshake::Ref> for Ref {
    type Error = error::RefConversion;

    fn try_from(r: handshake::Ref) -> Result<Self, Self::Error> {
        match r {
            handshake::Ref::Peeled {
                full_ref_name,
                tag,
                object,
            } => Ok(Ref {
                name: fullname_to_qualified(FullName::try_from(full_ref_name)?)?,
                target: Either::Left(tag),
                peeled: object,
            }),
            handshake::Ref::Direct {
                full_ref_name,
                object,
            } => Ok(Ref {
                name: fullname_to_qualified(FullName::try_from(full_ref_name)?)?,
                target: Either::Left(object),
                peeled: object,
            }),
            handshake::Ref::Symbolic {
                full_ref_name,
                target,
                object,
            } => Ok(Ref {
                name: fullname_to_qualified(FullName::try_from(full_ref_name)?)?,
                target: Either::Right(fullname_to_qualified(FullName::try_from(target)?)?),
                peeled: object,
            }),
            handshake::Ref::Unborn { full_ref_name, .. } => {
                Err(error::RefConversion::Unborn(full_ref_name))
            }
        }
    }
}

pub fn unpack_ref(r: handshake::Ref) -> (BString, ObjectId) {
    match r {
        handshake::Ref::Peeled {
            full_ref_name,
            object,
            ..
        }
        | handshake::Ref::Direct {
            full_ref_name,
            object,
        }
        | handshake::Ref::Symbolic {
            full_ref_name,
            object,
            ..
        } => (full_ref_name, object),
        handshake::Ref::Unborn { full_ref_name, .. } => {
            unreachable!("BUG: unborn ref {}", full_ref_name)
        }
    }
}

/// An iterator to scan for [`Ref`]s.
pub struct Scan<'a> {
    snapshot: &'a internal::Snapshot,
    odb: &'a Odb,
    inner: LooseThenPacked<'a, 'a>,
}

impl<'a> Scan<'a> {
    fn next_ref(&self, mut r: Reference) -> Result<Ref, error::Scan> {
        let peeled = self.snapshot.peel(self.odb, &mut r)?;
        let name = fullname_to_qualified(r.name)?;
        let target = match r.target {
            Target::Peeled(oid) => Either::Left(oid),
            Target::Symbolic(symref) => Either::Right(fullname_to_qualified(symref)?),
        };
        Ok(Ref {
            name,
            target,
            peeled,
        })
    }
}

impl<'a> Iterator for Scan<'a> {
    type Item = Result<Ref, error::Scan>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.next()?.map_err(error::Scan::from) {
            Ok(r) => Some(self.next_ref(r)),
            Err(e) => Some(Err(e)),
        }
    }
}

/// Handle to a Git reference store.
pub struct Refdb {
    /// The user information used for this Git repository.
    info: UserInfo,
    /// The shared object database for this reference database.
    odb: Odb,
    refdb: internal::Refdb,
    /// A snapshot of the reference store, using an mmapped packed-ref
    /// file -- providing the ability to efficiently search and
    /// iterate references.
    snapshot: internal::Snapshot,
}

impl Refdb {
    /// Create a new [`Refdb`] handle from a [`UserInfo`] and the path
    /// to the Git repository.
    pub fn new(info: UserInfo, git_dir: impl Into<PathBuf>) -> Result<Self, error::Init> {
        let git_dir = git_dir.into();
        let odb = Odb::new(&git_dir)?;
        let refdb = internal::Refdb::open(git_dir)?;
        let snapshot = refdb.snapshot()?;

        Ok(Self {
            info,
            odb,
            refdb,
            snapshot,
        })
    }

    /// See [`Odb::contains`].
    pub fn contains(&self, oid: impl AsRef<gix_hash::oid>) -> bool {
        self.odb.contains(oid)
    }

    /// See [`Odb::is_in_ancestry_path`].
    pub fn is_in_ancestry_path(
        &self,
        new: impl Into<ObjectId>,
        old: impl Into<ObjectId>,
    ) -> Result<bool, odb::error::Revwalk> {
        self.odb.is_in_ancestry_path(new, old)
    }

    /// Scan for references in the [`Refdb`], providing an optional
    /// `prefix` to filter by.
    pub fn scan(&self, prefix: Option<impl AsRef<Path>>) -> Result<Scan<'_>, error::Scan> {
        let inner = self.snapshot.iter(prefix)?;
        Ok(Scan {
            snapshot: &self.snapshot,
            odb: &self.odb,
            inner,
        })
    }

    /// Resolve the `refname` to its [`ObjectId`].
    ///
    /// If the `refname` does not exist in the [`Refdb`] then None is
    /// returned.
    pub fn refname_to_id<'a, N>(&self, refname: N) -> Result<Option<ObjectId>, error::Find>
    where
        N: Into<Qualified<'a>>,
    {
        let name = qualified_to_fullname(refname.into());
        match self.snapshot.find(name.as_ref().as_partial_name())? {
            None => Ok(None),
            Some(mut tip) => Ok(Some(self.snapshot.peel(&self.odb, &mut tip)?)),
        }
    }

    /// Reload the snapshot of the [`Refdb`].
    pub fn reload(&mut self) -> Result<(), error::Reload> {
        self.snapshot = self.refdb.snapshot()?;
        Ok(())
    }

    /// Apply the provided `updated` to the [`Refdb`].
    ///
    /// The result will have a set of successful and rejected changes.
    ///
    /// If there were new updates, the underlying snapshot is
    /// reloaded.
    pub fn update<'a, I>(&mut self, updates: I) -> Result<Applied<'a>, error::Update>
    where
        I: IntoIterator<Item = Update<'a>>,
    {
        let (rejected, edits) = updates
            .into_iter()
            .map(|update| self.to_edits(update))
            .filter_map(|r| r.ok())
            .fold(
                (
                    Vec::<Update<'a>>::default(),
                    HashMap::<FullName, Edit>::default(),
                ),
                |(mut rejected, mut edits), e| {
                    match e {
                        Left(r) => rejected.push(r),
                        Right(e) => edits.extend(e.into_iter().map(|e| (e.edit.name.clone(), e))),
                    }
                    (rejected, edits)
                },
            );

        let txn = self.snapshot.transaction().prepare(
            edits.clone().into_values().map(|e| e.edit),
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?;

        let signature = self.info.signature();
        let applied = txn
            .commit(Some(signature.to_ref()))?
            .into_iter()
            .map(|RefEdit { change, name, .. }| {
                let prev = edits
                    .get(&name)
                    .map(|e| e.prev)
                    .unwrap_or_else(|| panic!("BUG: edits are missing ref {name}"));
                let name = fullname_to_refstring(name)?;
                Ok(match change {
                    Change::Update { new, .. } => match new {
                        Target::Peeled(target) => Updated::Direct { name, prev, target },
                        Target::Symbolic(target) => Updated::Symbolic {
                            name,
                            prev,
                            target: fullname_to_refstring(target)?,
                        },
                    },
                    Change::Delete { .. } => Updated::Prune { name, prev },
                })
            })
            .collect::<Result<Vec<_>, error::Update>>()?;

        if !applied.is_empty() {
            self.reload()?;
        }

        Ok(Applied {
            rejected,
            updated: applied,
        })
    }

    fn to_edits<'a>(
        &self,
        update: Update<'a>,
    ) -> Result<Either<Update<'a>, Vec<Edit>>, error::Update> {
        match update {
            Update::Direct {
                name,
                target,
                no_ff,
            } => self.direct_edit(name, target, no_ff),
            Update::Symbolic {
                name,
                target,
                type_change,
            } => self.symbolic_edit(name, target, type_change),
            Update::Prune { name, prev } => Ok(Either::Right(vec![Edit {
                edit: RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::MustExistAndMatch(
                            prev.clone()
                                .map_right(qualified_to_fullname)
                                .either(Target::Peeled, Target::Symbolic),
                        ),
                        log: RefLog::AndReference,
                    },
                    name: namespaced_to_fullname(name),
                    deref: false,
                },
                prev: match prev {
                    Left(oid) => oid,
                    Right(name) => self
                        .refname_to_id(name.clone())?
                        .ok_or_else(|| error::Update::Missing(name.to_ref_string()))?,
                },
            }])),
        }
    }

    fn direct_edit<'a>(
        &self,
        name: Namespaced<'a>,
        target: ObjectId,
        no_ff: Policy,
    ) -> Result<Either<Update<'a>, Vec<Edit>>, error::Update> {
        use Either::*;

        let force_create_reflog = force_reflog(&name);
        let name_ns = namespaced_to_fullname(name.clone());
        let tip = self.find_snapshot(&name_ns)?;
        match tip {
            None => Ok(Right(vec![Edit {
                edit: RefEdit {
                    change: Change::Update {
                        log: LogChange {
                            mode: RefLog::AndReference,
                            force_create_reflog,
                            message: "radicle: create".into(),
                        },
                        expected: PreviousValue::MustNotExist,
                        new: Target::Peeled(target),
                    },
                    name: name_ns,
                    deref: false,
                },
                prev: oid::null(),
            }])),
            Some(prev) => {
                let is_ff = self.odb.is_in_ancestry_path(target, prev)?;

                if !is_ff {
                    match no_ff {
                        Policy::Abort => Err(error::Update::NonFF {
                            name: name_ns.into_inner(),
                            new: target,
                            cur: prev,
                        }),
                        Policy::Reject => Ok(Left(Update::Direct {
                            name,
                            target,
                            no_ff,
                        })),
                        Policy::Allow => Ok(Right(vec![Edit {
                            edit: RefEdit {
                                change: Change::Update {
                                    log: LogChange {
                                        mode: RefLog::AndReference,
                                        force_create_reflog,
                                        message: "radicle: forced update".into(),
                                    },
                                    expected: PreviousValue::MustExistAndMatch(Target::Peeled(
                                        prev,
                                    )),
                                    new: Target::Peeled(target),
                                },
                                name: name_ns,
                                deref: false,
                            },
                            prev,
                        }])),
                    }
                } else {
                    Ok(Right(vec![Edit {
                        edit: RefEdit {
                            change: Change::Update {
                                log: LogChange {
                                    mode: RefLog::AndReference,
                                    force_create_reflog,
                                    message: "radicle: fast-forward".into(),
                                },
                                expected: PreviousValue::MustExistAndMatch(Target::Peeled(prev)),
                                new: Target::Peeled(target),
                            },
                            name: name_ns,
                            deref: false,
                        },
                        prev,
                    }]))
                }
            }
        }
    }

    fn symbolic_edit<'a>(
        &self,
        name: Namespaced<'a>,
        target: SymrefTarget<'a>,
        type_change: Policy,
    ) -> Result<Either<Update<'a>, Vec<Edit>>, error::Update> {
        let name_ns = namespaced_to_fullname(name.clone());
        let src = self
            .snapshot
            .find(name_ns.as_bstr())
            .map_err(error::Find::from)?
            .map(|r| r.target);

        match src {
            Some(Target::Peeled(_)) if matches!(type_change, Policy::Abort) => {
                Err(error::Update::TypeChange(name_ns.into_inner()))
            }
            Some(Target::Peeled(_)) if matches!(type_change, Policy::Reject) => {
                Ok(Left(Update::Symbolic {
                    name,
                    target,
                    type_change,
                }))
            }

            _ => {
                let src_name = name_ns;
                let dst = self
                    .snapshot
                    .find(target.name().as_bstr())
                    .map_err(error::Find::from)?
                    .map(|r| r.target);

                let SymrefTarget {
                    name: dst_name,
                    target,
                } = target;
                let edits = match dst {
                    Some(Target::Symbolic(dst)) => {
                        return Err(error::Update::TargetSymbolic(dst.into_inner()))
                    }

                    None => {
                        let force_create_reflog = force_reflog(&dst_name);
                        let dst_name = namespaced_to_fullname(dst_name);
                        vec![
                            // Create target
                            Edit {
                                edit: RefEdit {
                                    change: Change::Update {
                                        log: LogChange {
                                            mode: RefLog::AndReference,
                                            force_create_reflog,
                                            message: "radicle: create symref target".into(),
                                        },
                                        expected: PreviousValue::MustNotExist,
                                        new: Target::Peeled(target),
                                    },
                                    name: dst_name.clone(),
                                    deref: false,
                                },
                                prev: oid::null(),
                            },
                            // Create source
                            Edit {
                                edit: RefEdit {
                                    change: Change::Update {
                                        log: LogChange {
                                            mode: RefLog::AndReference,
                                            force_create_reflog,
                                            message: "radicle: create symbolic ref".into(),
                                        },
                                        expected: PreviousValue::MustNotExist,
                                        new: Target::Symbolic(dst_name),
                                    },
                                    name: src_name,
                                    deref: false,
                                },
                                prev: oid::null(),
                            },
                        ]
                    }

                    Some(Target::Peeled(dst)) => {
                        let mut edits = Vec::with_capacity(2);

                        let is_ff = target != dst && self.odb.is_in_ancestry_path(target, dst)?;
                        let force_create_reflog = force_reflog(&dst_name);
                        let dst_name = namespaced_to_fullname(dst_name);

                        if is_ff {
                            edits.push(Edit {
                                edit: RefEdit {
                                    change: Change::Update {
                                        log: LogChange {
                                            mode: RefLog::AndReference,
                                            force_create_reflog,
                                            message: "radicle: fast-forward symref target".into(),
                                        },
                                        expected: PreviousValue::MustExistAndMatch(Target::Peeled(
                                            dst,
                                        )),
                                        new: Target::Peeled(target),
                                    },
                                    name: dst_name.clone(),
                                    deref: false,
                                },
                                prev: dst,
                            })
                        }

                        edits.push(Edit {
                            edit: RefEdit {
                                change: Change::Update {
                                    log: LogChange {
                                        mode: RefLog::AndReference,
                                        force_create_reflog,
                                        message: "radicle: symbolic ref".into(),
                                    },
                                    expected: src
                                        .map(PreviousValue::MustExistAndMatch)
                                        .unwrap_or(PreviousValue::MustNotExist),
                                    new: Target::Symbolic(dst_name),
                                },
                                name: src_name,
                                deref: false,
                            },
                            prev: dst,
                        });
                        edits
                    }
                };

                Ok(Right(edits))
            }
        }
    }

    fn find_snapshot(&self, name: &FullName) -> Result<Option<ObjectId>, error::Find> {
        match self.snapshot.find(name.as_ref().as_partial_name())? {
            None => Ok(None),
            Some(mut tip) => Ok(Some(self.snapshot.peel(&self.odb, &mut tip)?)),
        }
    }
}

//
// Helpers for converting from/to FullName and ref_format names.
//

fn fullname_to_qualified(name: FullName) -> Result<Qualified<'static>, git_ext::ref_format::Error> {
    fullname_to_refstring(name).map(|name| {
        name.into_qualified()
            .expect("refdb scan should always return qualified references")
    })
}

fn qualified_to_fullname(n: Qualified<'_>) -> FullName {
    let name = n.into_refstring().into_bstring();
    FullName::try_from(name).expect("`Namespaced` is a valid `FullName`")
}

fn namespaced_to_fullname(ns: Namespaced<'_>) -> FullName {
    qualified_to_fullname(ns.into_qualified())
}

fn fullname_to_refstring(name: FullName) -> Result<RefString, git_ext::ref_format::Error> {
    RefString::try_from(Vec::from(name.into_inner()).into_string_lossy())
}

fn force_reflog(ns: &Namespaced<'_>) -> bool {
    let refname = ns.strip_namespace();
    let (_refs, cat, _, _) = refname.non_empty_components();
    cat.as_str() == "rad"
}

mod oid {
    use gix_hash::{Kind::Sha1, ObjectId};

    /// Helper for providing the zero ObjectId under the Sha1 scheme
    pub fn null() -> ObjectId {
        ObjectId::null(Sha1)
    }
}
