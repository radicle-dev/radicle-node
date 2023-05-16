use std::str::FromStr;

use either::Either;
use git_ext::ref_format::{qualified, Component, Namespaced, Qualified, RefString};
use gix_hash::ObjectId;
use gix_object::bstr::{BStr, BString, ByteSlice};
use once_cell::sync::Lazy;
use radicle_crypto::PublicKey;
use thiserror::Error;

use crate::gix::refdb::{Policy, Update};

pub(crate) static REFS_RAD_ID: Lazy<Qualified<'static>> = Lazy::new(|| qualified!("refs/rad/id"));
pub(crate) static REFS_RAD_SIGREFS: Lazy<Qualified<'static>> =
    Lazy::new(|| qualified!("refs/rad/sigrefs"));

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("ref name '{0}' is not qualified")]
    NotQualified(RefString),

    #[error("non-namespaced ref name '{0}' is not 'refs/rad/id'")]
    NotCanonicalRadID(Qualified<'static>),

    #[error("ref name '{0}' is not namespaced")]
    NotNamespaced(Qualified<'static>),

    #[error("invalid remote peer id")]
    PublicKey(#[from] radicle_crypto::PublicKeyError),

    #[error("malformed ref name")]
    Check(#[from] git_ext::ref_format::Error),

    #[error("malformed ref name")]
    MalformedSuffix,

    #[error(transparent)]
    Utf8(#[from] bstr::Utf8Error),
}

/// The set of special references used in the Heartwood protocol.
#[derive(Clone, Copy, Debug)]
pub enum Special {
    /// `rad/id`
    Id,
    /// `rad/sigrefs`
    SignedRefs,
}

impl From<Special> for Qualified<'_> {
    fn from(s: Special) -> Self {
        match s {
            Special::Id => (*REFS_RAD_ID).clone(),
            Special::SignedRefs => (*REFS_RAD_SIGREFS).clone(),
        }
    }
}

/// A reference living under `refs/namesapces/<remote>`
#[derive(Debug)]
pub struct RemoteRef<'a> {
    /// The namespace of the remote.
    pub remote: PublicKey,
    /// The reference is expected to either be a [`Special`] reference
    /// or a generic reference name.
    pub suffix: Either<Special, Qualified<'a>>,
}

impl<'a> RemoteRef<'a> {
    pub fn is_special(&self) -> bool {
        self.suffix.is_left()
    }

    pub fn to_ref_string(&self) -> RefString {
        Namespaced::from(self).to_ref_string()
    }

    pub fn rad_id<'b>(remote: PublicKey) -> Namespaced<'b> {
        Namespaced::from(Self {
            remote,
            suffix: Either::Left(Special::Id),
        })
    }

    pub fn rad_sigrefs<'b>(remote: PublicKey) -> Namespaced<'b> {
        Namespaced::from(Self {
            remote,
            suffix: Either::Left(Special::SignedRefs),
        })
    }

    pub fn as_verification_ref_update(&self, tip: &ObjectId) -> Option<Update<'static>> {
        self.suffix.as_ref().left().map(|special| match special {
            Special::Id | Special::SignedRefs => Update::Direct {
                name: Qualified::from(*special).with_namespace(Component::from(&self.remote)),
                target: *tip,
                no_ff: Policy::Abort,
            },
        })
    }
}

impl<'a> From<RemoteRef<'a>> for Namespaced<'_> {
    fn from(r: RemoteRef) -> Self {
        Self::from(&r)
    }
}

impl<'a> From<&RemoteRef<'a>> for Namespaced<'_> {
    fn from(RemoteRef { remote, suffix }: &RemoteRef) -> Self {
        let ns = Component::from(remote);
        match suffix {
            Either::Left(special) => Qualified::from(*special).with_namespace(ns),
            Either::Right(refname) => refname.with_namespace(ns).to_owned(),
        }
    }
}

impl TryFrom<Qualified<'_>> for RemoteRef<'_> {
    type Error = Error;

    fn try_from(name: Qualified<'_>) -> Result<Self, Self::Error> {
        let ns = name
            .to_namespaced()
            .ok_or_else(|| Error::NotNamespaced(name.to_owned()))?;
        Self::try_from(ns)
    }
}

impl TryFrom<Namespaced<'_>> for RemoteRef<'_> {
    type Error = Error;

    fn try_from(ns: Namespaced<'_>) -> Result<Self, Self::Error> {
        fn parse_suffix<'a>(
            head: Component<'a>,
            mut iter: impl Iterator<Item = Component<'a>>,
        ) -> Option<Special> {
            match (head.as_str(), iter.next()) {
                ("id", None) => Some(Special::Id),
                ("sigrefs", None) => Some(Special::SignedRefs),
                _ => None,
            }
        }

        let remote = PublicKey::from_str(ns.namespace().as_str())?;
        let name = ns.strip_namespace();
        let suffix = match name.non_empty_components() {
            (_refs, cat, head, tail) if "rad" == cat.as_str() => {
                parse_suffix(head, tail).map(Either::Left)
            }
            _ => Some(Either::Right(name)),
        };
        Ok(Self {
            remote,
            suffix: suffix.ok_or(Error::MalformedSuffix)?,
        })
    }
}

/// A reference name received during an exchange with another peer. The
/// expected references are either namespaced references in the form
/// of [`RemoteRef`] or the canonical `rad/id` reference.
#[derive(Debug)]
pub enum Refname<'a> {
    /// A reference name under a `remote` namespace.
    ///
    /// # Examples
    ///
    ///   * `refs/namespaces/<remote>/refs/rad/id`
    ///   * `refs/namespaces/<remote>/refs/rad/sigrefs`
    ///   * `refs/namespaces/<remote>/refs/heads/main`
    ///   * `refs/namespaces/<remote>/refs/cobs/issue.rad.xyz`
    Namespaced(RemoteRef<'a>),
    /// The canonical `refs/rad/id` reference
    RadId,
}

impl<'a> Refname<'a> {
    pub fn as_namespaced_ref(&self) -> Option<&RemoteRef<'a>> {
        match self {
            Refname::Namespaced(ns) => Some(ns),
            Refname::RadId => None,
        }
    }

    pub fn remote(remote: PublicKey, suffix: Qualified<'a>) -> Self {
        let ns = RemoteRef {
            remote,
            suffix: Either::Right(suffix),
        };
        Self::Namespaced(ns)
    }

    pub fn to_qualified<'b>(&self) -> Qualified<'b> {
        match &self {
            Self::Namespaced(RemoteRef { remote, suffix }) => match suffix {
                Either::Left(s) => Qualified::from(*s)
                    .with_namespace(Component::from(remote))
                    .into(),
                Either::Right(name) => {
                    Qualified::from(name.with_namespace(Component::from(remote))).to_owned()
                }
            },
            Self::RadId => REFS_RAD_ID.clone(),
        }
    }

    pub fn to_namespaced<'b>(&self) -> Option<Namespaced<'b>> {
        match self {
            Self::Namespaced(ns) => Some(ns.into()),
            Self::RadId => None,
        }
    }
}

impl TryFrom<BString> for Refname<'_> {
    type Error = Error;

    fn try_from(value: BString) -> Result<Self, Self::Error> {
        let name = RefString::try_from(value.to_str()?)?;
        Self::try_from(name)
    }
}

impl TryFrom<&BStr> for Refname<'_> {
    type Error = Error;

    fn try_from(value: &BStr) -> Result<Self, Self::Error> {
        let name = RefString::try_from(value.to_str()?)?;
        Self::try_from(name)
    }
}

impl TryFrom<RefString> for Refname<'_> {
    type Error = Error;

    fn try_from(r: RefString) -> Result<Self, Self::Error> {
        r.clone()
            .into_qualified()
            .ok_or(Error::NotQualified(r))
            .and_then(Self::try_from)
    }
}

impl<'a> TryFrom<Qualified<'a>> for Refname<'_> {
    type Error = Error;

    fn try_from(name: Qualified<'a>) -> Result<Self, Self::Error> {
        match name.to_namespaced() {
            Some(ns) => RemoteRef::try_from(ns).map(Self::Namespaced),
            None => {
                if name == *REFS_RAD_ID {
                    Ok(Refname::RadId)
                } else {
                    Err(Error::NotCanonicalRadID(name.to_owned()))
                }
            }
        }
    }
}

/// A reference name and the associated tip received during an
/// exchange with another peer.
#[derive(Debug)]
pub struct ReceivedRef {
    pub tip: ObjectId,
    pub name: Refname<'static>,
}

impl ReceivedRef {
    pub fn new(tip: ObjectId, name: Refname<'static>) -> Self {
        Self { tip, name }
    }

    pub fn to_qualified(&self) -> Qualified<'static> {
        self.name.to_qualified()
    }

    pub fn as_verification_ref_update(&self) -> Option<Update<'static>> {
        self.name
            .as_namespaced_ref()
            .and_then(|RemoteRef { remote, suffix }| {
                suffix.as_ref().left().map(|special| match special {
                    Special::Id | Special::SignedRefs => Update::Direct {
                        name: Qualified::from(*special).with_namespace(Component::from(remote)),
                        target: self.tip,
                        no_ff: Policy::Abort,
                    },
                })
            })
    }
}
