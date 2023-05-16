use std::collections::{BTreeMap, BTreeSet, HashSet};

use bstr::BString;
use either::Either;
use git_ext::ref_format::{refname, Component, Namespaced, Qualified};
use gix_hash::ObjectId;
use gix_protocol::handshake::Ref;
use nonempty::NonEmpty;
use radicle_crypto::PublicKey;

use crate::gix::refdb;
use crate::gix::refdb::{Policy, Refdb, Update, Updates};
use crate::identity::{Identities, Verified as _};
use crate::protocol::AsClone;
use crate::refs::{ReceivedRef, Refname, RemoteRef};
use crate::state::FetchState;
use crate::transport::{WantsHaves, WantsHavesBuilder};
use crate::{refs, tracking};
use crate::{sigrefs, transport};

pub mod error {
    use radicle_crypto::PublicKey;
    use radicle_git_ext::ref_format::RefString;
    use thiserror::Error;

    use crate::transport::WantsHavesError;

    #[derive(Debug, Error)]
    pub enum Layout {
        #[error("missing required refs: {0:?}")]
        MissingRequiredRefs(Vec<String>),
    }

    #[derive(Debug, Error)]
    pub enum Prepare {
        #[error("refdb scan error")]
        Scan {
            #[source]
            err: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
        #[error("verification of rad/id for {remote} failed")]
        Verification {
            remote: PublicKey,
            #[source]
            err: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
    }

    #[derive(Debug, Error)]
    pub enum WantsHaves {
        #[error(transparent)]
        WantsHavesAdd(#[from] WantsHavesError),
        #[error("expected namespaced ref {0}")]
        NotNamespaced(RefString),
    }
}

pub(crate) trait Step {
    /// Validate that all advertised refs conform to an expected layout.
    ///
    /// The supplied `refs` are `ls-ref`-advertised refs filtered
    /// through [`Step::ref_filter`].
    fn pre_validate(&self, refs: &[ReceivedRef]) -> Result<(), error::Layout>;

    /// If and how to perform `ls-refs`.
    fn ls_refs(&self) -> Option<NonEmpty<BString>>;

    /// Filter a remote-advertised [`Ref`].
    ///
    /// Return `Some` if the ref should be considered, `None` otherwise. This
    /// method may be called with the response of `ls-refs`, the `wanted-refs`
    /// of a `fetch` response, or both.
    fn ref_filter(&self, r: Ref) -> Option<ReceivedRef>;

    /// Assemble the `want`s and `have`s for a `fetch`, retaining the refs which
    /// would need updating after the `fetch` succeeds.
    ///
    /// The `refs` are the advertised refs from executing `ls-refs`, filtered
    /// through [`Step::ref_filter`].
    fn wants_haves(
        &self,
        refdb: &Refdb,
        refs: &[ReceivedRef],
    ) -> Result<Option<WantsHaves>, error::WantsHaves> {
        let mut builder = WantsHavesBuilder::default();
        builder.add(refdb, refs)?;
        Ok(builder.build())
    }

    /// Prepare the [`Updates`] based on the received `refs`.
    ///
    /// These updates can then be used to update the refdb.
    fn prepare<'a, I>(
        &self,
        s: &FetchState,
        refdb: &Refdb,
        ids: &I,
        refs: &'a [ReceivedRef],
    ) -> Result<Updates<'a>, error::Prepare>
    where
        I: Identities;
}

/// The [`Step`] for performing an initial clone from a `remote`.
///
/// This step asks for the canonical `refs/rad/id` reference, which
/// allows us to use it as an anchor for the following steps.
#[derive(Debug)]
pub struct Clone {
    pub remote: PublicKey,
    pub limit: u64,
}

impl Step for Clone {
    fn pre_validate(&self, refs: &[ReceivedRef]) -> Result<(), error::Layout> {
        // Ensures that we fetched the canonical 'refs/rad/id'
        ensure_refs(
            [BString::from(refs::REFS_RAD_ID.as_bstr())]
                .into_iter()
                .collect(),
            refs.iter()
                .map(|r| r.to_qualified().to_string().into())
                .collect(),
        )
    }

    fn ls_refs(&self) -> Option<NonEmpty<BString>> {
        Some(NonEmpty::new(refs::REFS_RAD_ID.as_bstr().into()))
    }

    fn ref_filter(&self, r: Ref) -> Option<ReceivedRef> {
        let (name, tip) = refdb::unpack_ref(r);
        match refs::Refname::try_from(name).ok()? {
            refname @ Refname::Namespaced(RemoteRef {
                suffix: Either::Left(_),
                ..
            }) => Some(ReceivedRef::new(tip, refname)),
            Refname::RadId => Some(ReceivedRef::new(tip, Refname::RadId)),
            _ => None,
        }
    }

    fn prepare<'a, I>(
        &self,
        s: &FetchState,
        _refdb: &Refdb,
        ids: &I,
        refs: &'a [ReceivedRef],
    ) -> Result<Updates<'a>, error::Prepare>
    where
        I: Identities,
    {
        let verified = ids
            .verified(
                *s.canonical_id()
                    .expect("ensured we got canonical rad/id ref"),
            )
            .map_err(|err| error::Prepare::Verification {
                remote: self.remote,
                err: Box::new(err),
            })?;
        let tips = if verified.delegates().contains(&self.remote) {
            refs.iter()
                .filter_map(ReceivedRef::as_verification_ref_update)
                .collect()
        } else {
            vec![]
        };

        Ok(Updates { tips })
    }
}

/// The [`Step`] for fetching verification refs from the set of
/// remotes in `tracked` and `delegates`.
///
/// This step asks for all tracked and delegate remote's `rad/id` and
/// `rad/sigrefs`, iff the scope is
/// [`tracking::Scope::Trusted`]. Otherwise, it asks for all
/// namespaces.
///
/// It ensures that all delegate refs were fetched.
#[derive(Debug)]
pub struct Fetch {
    pub local: AsClone,
    pub remote: PublicKey,
    pub tracked: tracking::Tracked,
    pub delegates: BTreeSet<PublicKey>,
    pub limit: u64,
}

impl Step for Fetch {
    fn pre_validate(&self, refs: &[ReceivedRef]) -> Result<(), error::Layout> {
        ensure_refs(
            self.delegates
                .iter()
                .filter(|id| self.local.should_keep(id))
                .flat_map(|id| special_refs(*id))
                .collect(),
            refs.iter()
                .filter_map(|r| r.name.as_namespaced_ref())
                .map(|r| Namespaced::from(r).to_string().into())
                .collect(),
        )
    }

    fn ls_refs(&self) -> Option<NonEmpty<BString>> {
        match &self.tracked.scope {
            tracking::Scope::All => Some(NonEmpty::new("refs/namespaces".into())),
            tracking::Scope::Trusted => NonEmpty::collect(
                self.tracked
                    .remotes
                    .iter()
                    .chain(self.delegates.iter())
                    .flat_map(|remote| special_refs(*remote)),
            ),
        }
    }

    fn ref_filter(&self, r: Ref) -> Option<ReceivedRef> {
        let (refname, tip) = refdb::unpack_ref(r);
        let refname = refs::Refname::try_from(refname).ok()?;
        refname
            .as_namespaced_ref()?
            .is_special()
            .then_some(ReceivedRef::new(tip, refname))
    }

    fn prepare<'a, I>(
        &self,
        _s: &FetchState,
        _refdb: &Refdb,
        ids: &I,
        refs: &'a [ReceivedRef],
    ) -> Result<Updates<'a>, error::Prepare>
    where
        I: Identities,
    {
        verification_refs(&self.local, ids, refs, |remote_id| {
            self.delegates.contains(remote_id)
        })
    }
}

/// The [`Step`] for fetching data refs from the set of
/// remotes in `trusted`.
///
/// All refs that are listed in the `trusted` sigrefs are checked
/// against our refdb/odb to build a set of `wants` and `haves`. The
/// `wants` will then be fetched from the server side to receive those
/// particular objects.
///
/// Those refs and objects are then prepared for updating, removing
/// any that were found to exist before the latest fetch.
#[derive(Debug)]
pub struct Refs {
    pub remote: PublicKey,
    pub trusted: sigrefs::RemoteRefs,
    pub limit: u64,
}

impl Step for Refs {
    fn pre_validate(&self, _refs: &[ReceivedRef]) -> Result<(), error::Layout> {
        Ok(())
    }

    fn ls_refs(&self) -> Option<NonEmpty<BString>> {
        NonEmpty::collect(self.trusted.keys().flat_map(|remote| special_refs(*remote)))
    }

    fn ref_filter(&self, r: Ref) -> Option<ReceivedRef> {
        let (name, tip) = refdb::unpack_ref(r);
        match refs::Refname::try_from(name).ok()? {
            Refname::Namespaced(RemoteRef { remote, suffix })
                if suffix.is_left() && self.trusted.contains_key(&remote) =>
            {
                Some(ReceivedRef::new(
                    tip,
                    Refname::Namespaced(RemoteRef { remote, suffix }),
                ))
            }
            Refname::RadId => Some(ReceivedRef::new(tip, Refname::RadId)),
            _ => None,
        }
    }

    fn wants_haves(
        &self,
        refdb: &Refdb,
        refs: &[ReceivedRef],
    ) -> Result<Option<WantsHaves>, error::WantsHaves> {
        let mut builder = WantsHavesBuilder::default();

        for (remote, refs) in &self.trusted {
            for (name, tip) in refs {
                let refname = Qualified::from_refstr(name)
                    .and_then(|suffix| refs::Refname::remote(*remote, suffix).to_namespaced())
                    .ok_or_else(|| error::WantsHaves::NotNamespaced(name.to_owned()))?;
                let want = match refdb
                    .refname_to_id(refname)
                    .map_err(transport::WantsHavesError::from)?
                {
                    Some(oid) => {
                        let want = *tip != oid && !refdb.contains(tip);
                        builder.have(oid);
                        want
                    }
                    None => !refdb.contains(tip),
                };
                if want {
                    builder.want(*tip);
                }
            }
        }

        builder.add(refdb, refs)?;
        Ok(builder.build())
    }

    fn prepare<'a, I>(
        &self,
        _s: &FetchState,
        refdb: &Refdb,
        _ids: &I,
        _refs: &'a [ReceivedRef],
    ) -> Result<Updates<'a>, error::Prepare>
    where
        I: Identities,
    {
        let mut tips = {
            let sz = self.trusted.values().map(|rs| rs.refs.len()).sum();
            Vec::with_capacity(sz)
        };

        for (remote, refs) in &self.trusted {
            let mut signed = HashSet::with_capacity(refs.refs.len());
            for (name, tip) in refs {
                let tracking: Namespaced<'_> = Qualified::from_refstr(name)
                    .and_then(|q| refs::Refname::remote(*remote, q).to_namespaced())
                    .expect("we checked sigrefs well-formedness in wants_refs already");
                signed.insert(tracking.clone());
                tips.push(Update::Direct {
                    name: tracking,
                    target: tip.as_ref().to_owned(),
                    no_ff: Policy::Allow,
                });
            }

            // Prune refs not in signed
            let prefix = refname!("refs/namespaces").join(Component::from(remote));
            let prefix_rad = prefix.join(refname!("refs/rad"));
            let scan_err = |e: refdb::error::Scan| error::Prepare::Scan { err: e.into() };
            for known in refdb.scan(Some(prefix.as_str())).map_err(scan_err)? {
                let refdb::Ref { name, target, .. } = known.map_err(scan_err)?;
                let ns = name.to_namespaced();
                // should only be pruning namespaced refs
                let ns = match ns {
                    Some(name) => name.to_owned(),
                    None => continue,
                };

                // 'rad/' refs are never subject to pruning
                if ns.starts_with(prefix_rad.as_str()) {
                    continue;
                }

                if !signed.contains(&ns) {
                    tips.push(Update::Prune {
                        name: ns,
                        prev: target,
                    });
                }
            }
        }

        Ok(Updates { tips })
    }
}

fn verification_refs<'a, F>(
    local_id: &AsClone,
    ids: &impl Identities,
    refs: &'a [ReceivedRef],
    is_delegate: F,
) -> Result<Updates<'a>, error::Prepare>
where
    F: Fn(&PublicKey) -> bool,
{
    use either::Either::*;

    let grouped: BTreeMap<&PublicKey, Vec<(ObjectId, &RemoteRef<'static>)>> = refs
        .iter()
        .filter_map(|r| {
            r.name.as_namespaced_ref().and_then(|name| {
                let remote_id = &name.remote;
                (local_id.should_keep(remote_id)).then_some((remote_id, r.tip, name))
            })
        })
        .fold(BTreeMap::new(), |mut acc, (remote_id, tip, name)| {
            acc.entry(remote_id)
                .or_insert_with(Vec::new)
                .push((tip, name));
            acc
        });

    let mut updates = Updates {
        tips: Vec::with_capacity(refs.len()),
    };

    for (remote_id, refs) in grouped {
        let is_delegate = is_delegate(remote_id);

        let mut tips_inner = BTreeMap::new();
        for (tip, name) in &refs {
            match &name.suffix {
                Left(refs::Special::Id) => {
                    match ids.verified(*tip) {
                        Err(e) if is_delegate => {
                            return Err(error::Prepare::Verification {
                                remote: *remote_id,
                                err: e.into(),
                            })
                        }
                        Err(e) => {
                            log::warn!("error verifying non-delegate id {remote_id}: {e}");
                            // Verification error for a non-delegate taints
                            // all refs for this remote_id
                            tips_inner.clear();
                            break;
                        }

                        Ok(_) => {
                            if let Some(u) = name.as_verification_ref_update(tip) {
                                tips_inner
                                    .entry(remote_id)
                                    .and_modify(|sr: &mut SpecialRefs<Option<Update>>| {
                                        sr.id = Some(u.clone());
                                    })
                                    .or_insert(SpecialRefs {
                                        id: Some(u),
                                        sigrefs: None,
                                    });
                            }
                        }
                    }
                }

                Left(refs::Special::SignedRefs) => {
                    if let Some(u) = name.as_verification_ref_update(tip) {
                        tips_inner
                            .entry(remote_id)
                            .and_modify(|sr| {
                                sr.sigrefs = Some(u.clone());
                            })
                            .or_insert(SpecialRefs {
                                id: None,
                                sigrefs: Some(u),
                            });
                    }
                }

                Right(_) => continue,
            }
        }

        let mut tips_inner = tips_inner
            .into_values()
            .filter_map(|sr| sr.verify())
            .flat_map(SpecialRefs::unpack)
            .collect::<Vec<_>>();
        updates.tips.append(&mut tips_inner);
    }

    Ok(updates)
}

fn special_refs(remote: PublicKey) -> impl Iterator<Item = BString> {
    [
        refs::RemoteRef::rad_id(remote).to_string().into(),
        refs::RemoteRef::rad_sigrefs(remote).to_string().into(),
    ]
    .into_iter()
}

fn ensure_refs<T>(required: BTreeSet<T>, wants: BTreeSet<T>) -> Result<(), error::Layout>
where
    T: Ord + ToString,
{
    if wants.is_empty() {
        return Ok(());
    }

    let diff = required.difference(&wants).collect::<Vec<_>>();

    if diff.is_empty() {
        Ok(())
    } else {
        Err(error::Layout::MissingRequiredRefs(
            diff.into_iter().map(|ns| ns.to_string()).collect(),
        ))
    }
}

/// Ensure that we have seen both the `rad/id` and `rad/sigrefs`
/// references.
struct SpecialRefs<T> {
    id: T,
    sigrefs: T,
}

impl<T> SpecialRefs<Option<T>> {
    /// If both `id` and `sigrefs` are `Some`, then the result is
    /// `Some`, otherwise it's `None`.
    fn verify(self) -> Option<SpecialRefs<T>> {
        self.id
            .zip(self.sigrefs)
            .map(|(id, sigrefs)| SpecialRefs { id, sigrefs })
    }
}

impl<T> SpecialRefs<T> {
    fn unpack(self) -> [T; 2] {
        [self.id, self.sigrefs]
    }
}
