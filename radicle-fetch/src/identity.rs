use std::fmt;

use git_ext::ref_format::Component;
use gix_hash::ObjectId;
use nonempty::NonEmpty;
use radicle_crypto::PublicKey;
use thiserror::Error;

use crate::gix::refdb;
use crate::refs;
use crate::state::Cached;

#[derive(Debug, Error)]
pub enum Error<E: std::error::Error + Send + Sync + 'static> {
    #[error(transparent)]
    Find(#[from] refdb::error::Find),

    #[error(transparent)]
    Verified(E),
}

/// A verified identity.
pub trait Verified {
    /// The identity's content identifier for referencing it.
    fn content_id(&self) -> ObjectId;

    /// The identity's current revision.
    fn revision(&self) -> ObjectId;

    /// The identity's set of delegates.
    fn delegates(&self) -> NonEmpty<PublicKey>;
}

pub trait Identities {
    type VerifiedIdentity: Verified + fmt::Debug + Send + Sync + 'static;

    type VerifiedError: std::error::Error + Send + Sync + 'static;

    /// Return the verified identity found at `head`.
    fn verified(&self, head: ObjectId) -> Result<Self::VerifiedIdentity, Self::VerifiedError>;

    /// Return the more recent of identities `a` and `b`, or an error if their
    /// histories are unrelated.
    fn newer(
        &self,
        a: Self::VerifiedIdentity,
        b: Self::VerifiedIdentity,
    ) -> Result<Self::VerifiedIdentity, error::History<Self::VerifiedIdentity>>;
}

/// Get an up-to-date identity.
///
/// The references that are inspected for the identity are in the
/// following order:
///   1. `refs/namespaces/{local}/refs/rad/id`
///   2. `refs/rad/id`
pub(crate) fn current<G, C, S>(
    cached: &Cached<G, C, S>,
    local: &PublicKey,
) -> Result<Option<C::VerifiedIdentity>, Error<C::VerifiedError>>
where
    C: Identities,
{
    let rad_id = refs::REFS_RAD_ID.with_namespace(Component::from(local));
    let tip = cached
        .refname_to_id(rad_id)
        .transpose()
        .or_else(|| cached.refname_to_id(refs::REFS_RAD_ID.clone()).transpose())
        .transpose()?;
    let cached_tip = cached.canonical_rad_id();

    tip.or(cached_tip)
        .map(|tip| cached.verified(tip).map_err(Error::Verified))
        .transpose()
}

/// Check if their is an update to the repository that the `local`
/// peer needs to confirm.
pub fn requires_confirmation<I>(
    identities: &I,
    local: &PublicKey,
    ours: Option<I::VerifiedIdentity>,
    theirs: I::VerifiedIdentity,
) -> Result<bool, error::History<I::VerifiedIdentity>>
where
    I: Identities,
{
    match ours {
        // `rad/id` exists, delegates to local, and is not at the same
        // revision as `theirs`
        Some(ours) if ours.delegates().contains(local) && ours.revision() != theirs.revision() => {
            // Check which one is more recent
            let tip = ours.content_id();
            let newer = identities.newer(ours, theirs)?;
            // Theirs is ahead, so we need to confirm.
            if newer.content_id().as_ref() != tip.as_ref() {
                Ok(true)
            }
            // Ours is ahead, nothing to do.
            else {
                Ok(false)
            }
        }
        // There is no local identity for us to check for change
        // proposals.
        _ => Ok(false),
    }
}

// TODO(finto): we also have the top-level error here and boxed errors
// below. Should consolidate all of these.
pub mod error {
    use std::fmt::Debug;

    use radicle_git_ext::ref_format::Namespaced;
    use thiserror::Error;

    use crate::gix::refdb;

    #[derive(Debug, Error)]
    pub enum Newest<I: Debug + Send + Sync + 'static> {
        #[error(transparent)]
        History(#[from] History<I>),
    }

    #[derive(Debug, Error)]
    pub enum Setup {
        #[error(transparent)]
        Find(#[from] refdb::error::Find),

        #[error(transparent)]
        Verify(Box<dyn std::error::Error + Send + Sync + 'static>),

        #[error("rad::setup: missing {refname}")]
        Missing { refname: Namespaced<'static> },
    }

    #[derive(Debug, Error)]
    pub enum History<I: Debug + Send + Sync + 'static> {
        #[error("identities are forked")]
        Fork { left: I, right: I },

        #[error(transparent)]
        Other(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
    }
}

/// Read and verify the identity at `refs/namespaces/<remote>/refs/rad/id` if the
/// current namespace.
///
/// If the ref `refs/namespaces/<remote>/refs/rad/id` is not found,
/// `None` is returned.
pub(crate) fn of<'a, G, C, S>(
    cached: &Cached<'a, G, C, S>,
    remote: &PublicKey,
) -> Result<Option<C::VerifiedIdentity>, Box<dyn std::error::Error + Send + Sync + 'static>>
where
    C: Identities,
{
    let id_ref = refs::RemoteRef::rad_id(*remote);
    let id = cached
        .refname_to_id(id_ref)?
        .map(|tip| cached.verified(tip))
        .transpose()?;
    Ok(id)
}

/// Read and verify the identities `of` peers.
///
/// Also determine which one is the most recent, or report an error if their
/// histories diverge.
///
/// If one of the remote tracking branches is not found, an error is returned.
/// If the id is equal to `local`, the [`VerifiedIdentity`] is read
/// via [`current`], otherwise via [`of`].
///
/// If the iterator `of` is empty, `None` is returned.
#[allow(clippy::type_complexity)]
pub(crate) fn newest<'a, G, C, S, I>(
    cached: &Cached<'a, G, C, S>,
    local: &PublicKey,
    of: I,
) -> Result<
    Option<(&'a PublicKey, C::VerifiedIdentity)>,
    Box<dyn std::error::Error + Send + Sync + 'static>,
>
where
    C: Identities,
    I: IntoIterator<Item = &'a PublicKey>,
{
    let mut newest = None;
    for id in of {
        let a = if id == local {
            self::current(cached, local)?.ok_or(format!(
                "`newest: missing `refs/namespaces/{}/refs/rad/id` or `refs/rad/id`",
                local
            ))?
        } else {
            self::of(cached, id)?.ok_or(format!(
                "newest: missing delegation id ref `refs/namespaces/{}/refs/rad/id`",
                id
            ))?
        };
        match newest {
            None => newest = Some((id, a)),
            Some((id_b, b)) => {
                let oid_b = b.content_id();
                let newer = cached.newer(a, b)?;
                if newer.content_id() != oid_b {
                    newest = Some((id, newer));
                } else {
                    newest = Some((id_b, newer));
                }
            }
        }
    }

    Ok(newest)
}
