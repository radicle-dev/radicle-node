use std::collections::HashSet;
use std::path::Path;

use either::Either;
use gix_hash::ObjectId;
use radicle_crypto::PublicKey;
use radicle_git_ext::ref_format::{Namespaced, Qualified, RefString};
use thiserror::Error;

use crate::gix::refdb;
use crate::gix::{refdb::Ref, Refdb};
use crate::refs::{self, RemoteRef, Special};
use crate::sigrefs::Sigrefs;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Scan(#[from] refdb::error::Scan),
}

#[derive(Debug, Error)]
pub enum Validation {
    #[error("{0} was not found in the signed refs")]
    AdditionalRef(Qualified<'static>),

    #[error("'{name}' is malformed")]
    BadRef {
        name: RefString,
        #[source]
        error: refs::Error,
    },

    #[error("{refname}: expected {expected}, but found {actual}")]
    MismatchedRef {
        expected: ObjectId,
        actual: ObjectId,
        refname: RefString,
    },

    #[error("missing `refs/namespaces/{0}/refs/rad/id`")]
    MissingRadId(PublicKey),

    #[error("missing `refs/namespaces/{0}/refs/rad/sigrefs`")]
    MissingRadSigRefs(PublicKey),

    #[error("missing `refs/namespaces/{remote}/{refname}`")]
    MissingRef {
        refname: RefString,
        remote: PublicKey,
    },

    #[error("no references found for {0}")]
    NoData(PublicKey),
}

/// Validate the `sigrefs` against the `remote`'s references found in
/// `refdb`.
pub fn validate(
    refdb: &Refdb,
    remote: PublicKey,
    sigrefs: &Sigrefs,
) -> Result<Vec<Validation>, Error> {
    let prefix = Path::new("refs/namespaces").join(remote.to_human());
    let refs = refdb.scan(Some(prefix))?;

    let mut failures = Vec::new();
    let mut seen = HashSet::new();
    let mut has_rad_id = false;
    let mut has_rad_sigrefs = false;
    let mut has_data = false;

    for r in refs {
        has_data = true;
        let Ref {
            name, peeled: oid, ..
        } = r?;
        log::debug!(target: "fetch", "validation seen: {name}");
        match RemoteRef::try_from(name.clone()) {
            Err(e) => failures.push(Validation::BadRef {
                name: name.to_ref_string(),
                error: e,
            }),
            Ok(refname) if refname.remote != remote => {
                log::warn!("skipping {} not owned by {}", refname.remote, remote)
            }
            Ok(refname) => {
                seen.insert(
                    Namespaced::from(&refname)
                        .strip_namespace()
                        .into_refstring(),
                );
                match refname.suffix {
                    Either::Left(Special::Id) => {
                        has_rad_id = true;
                    }
                    Either::Left(Special::SignedRefs) => {
                        has_rad_sigrefs = true;
                        if oid.as_ref() != sigrefs.at.as_ref() {
                            failures.push(Validation::MismatchedRef {
                                expected: sigrefs.at.to_owned(),
                                actual: oid.to_owned(),
                                refname: refname.to_ref_string(),
                            })
                        }
                    }

                    Either::Right(ref name) => match sigrefs.refs.get(&name.to_ref_string()) {
                        Some(tip) => {
                            if tip != &oid {
                                failures.push(Validation::MismatchedRef {
                                    expected: *tip,
                                    actual: oid,
                                    refname: refname.to_ref_string(),
                                })
                            }
                        }
                        None => {
                            failures.push(Validation::AdditionalRef(name.clone()));
                        }
                    },
                }
            }
        }
    }

    if !has_data {
        failures.push(Validation::NoData(remote));
    } else {
        if !has_rad_id {
            failures.push(Validation::MissingRadId(remote))
        }

        if !has_rad_sigrefs {
            failures.push(Validation::MissingRadSigRefs(remote))
        }

        for missing in sigrefs
            .refs
            .keys()
            .filter(|refname| !seen.contains(*refname))
        {
            failures.push(Validation::MissingRef {
                refname: missing.to_owned(),
                remote,
            })
        }
    }

    Ok(failures)
}
