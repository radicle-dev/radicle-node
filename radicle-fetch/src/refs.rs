use std::{iter, str::FromStr};

use either::Either;
use radicle_crypto::PublicKey;
use radicle_git_ext::ref_format::{
    lit, name::str::NAMESPACES, qualified, Component, Namespaced, Qualified, RefString,
};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("ref name '{0}' is not qualified")]
    NotQualified(RefString),

    #[error("ref name '{0}' is not namespaced")]
    NotNamespaced(Qualified<'static>),

    #[error("invalid remote peer id")]
    PublicKey(#[from] radicle_crypto::Error),

    #[error("malformed ref name")]
    Check(#[from] git_ref_format::Error),

    #[error(transparent)]
    Utf8(#[from] bstr::Utf8Error),
}

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
            Special::Id => qualified!("refs/rad/id"),
            Special::SignedRefs => qualified!("refs/rad/sigrefs"),
        }
    }
}

/// A parsed reference name under a `remote` namespace.
///
/// # Examples
///
///   * `refs/namespaces/<remote>/refs/rad/id`
///   * `refs/namespaces/<remote>/refs/rad/sigrefs`
///   * `refs/namespaces/<remote>/refs/heads/main`
///   * `refs/namespaces/<remote>/refs/cobs/issue.rad.xyz`
pub struct Refname<'a> {
    pub remote: PublicKey,
    pub suffix: Either<Special, Qualified<'a>>,
}

impl<'a> Refname<'a> {
    pub fn remote(remote: PublicKey, suffix: Qualified<'_>) -> Self {
        Self {
            remote,
            suffix: Either::Right(suffix),
        }
    }

    pub fn rad_id(remote: PublicKey) -> Self {
        Self {
            remote,
            suffix: Either::Left(Special::Id),
        }
        .namespaced()
    }

    pub fn rad_sigrefs(remote: PublicKey) -> Self {
        Self {
            remote,
            suffix: Either::Left(Special::SignedRefs),
        }
        .namespaced()
    }

    pub fn to_qualified<'b>(&self) -> Qualified<'b> {
        match &self.suffix {
            Either::Left(s) => (*s).into(),
            Either::Right(name) => name.clone().into_owned(),
        }
    }

    pub fn namespaced<'b>(&self) -> Namespaced<'b> {
        let ns = &self.remote;
        self.to_qualified().with_namespace(Component::from(ns))
    }
}

impl TryFrom<&BStr> for Refname<'_> {
    type Error = Error;

    fn try_from(value: &BStr) -> Result<Self, Self::Error> {
        let name = RefString::try_from(input.to_str()?)?;
        Self::try_from(name)
    }
}

impl TryFrom<RefString> for Refname<'_> {
    type Error = Error;

    fn try_from(r: RefString) -> Result<Self, Self::Error> {
        r.into_qualified()
            .ok_or_else(|| Error::NotQualified(r))
            .and_then(Self::try_from)
    }
}

impl<'a> TryFrom<Qualified<'a>> for Refname<'_> {
    type Error = Error;

    fn try_from(name: Qualified<'a>) -> Result<Self, Self::Error> {
        let ns = name
            .to_namespaced()
            .ok_or_else(|| Error::NotNamespaced(name))?;
        let remote = PublicKey::from_str(ns.namespace().as_str())?;
        let name = ns.strip_namespace();
        let suffix = match name.non_empty_components() {
            (_refs, cat, head, mut tail) if "rad" == cat.as_str() => {
                match (tail.next(), tail.next()) {
                    ("id", None) => Some(Either::Left(Special::Id)),
                    ("sigrefs", None) => Some(Either::Left(Special::SignedRefs)),
                    _ => None,
                }
            }
            _ => Some(Either::Right(name)),
        };
        Ok(Refname {
            remote,
            suffix: suffix.ok_or(Error::MalformedSuffix)?,
        })
    }
}
