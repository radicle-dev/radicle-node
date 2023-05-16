use std::collections::{hash_map, HashMap};
use std::path::Path;

use either::Either;
use git_ext::ref_format::Qualified;
use gix_hash::ObjectId;

use super::{Applied, Ref, Update, Updated};

/// An in-memory reference store.
///
/// It provides the same functionality as the [`super::Refdb`], but is
/// used to temporarily store reference names and objects.
#[derive(Clone, Debug, Default)]
pub struct InMemory(HashMap<Qualified<'static>, ObjectId>);

impl InMemory {
    pub fn scan(&self, prefix: Option<impl AsRef<Path>>) -> Scan<'_> {
        Scan {
            prefix: prefix.map(|p| p.as_ref().to_string_lossy().into_owned()),
            inner: self.0.iter(),
        }
    }

    pub fn refname_to_id<'a, N>(&self, refname: N) -> Option<ObjectId>
    where
        N: Into<Qualified<'a>>,
    {
        let name = refname.into();
        self.0.get(&name).copied()
    }

    pub fn reload(&mut self) {}

    pub fn update<'a, I>(&mut self, updates: I) -> Applied<'a>
    where
        I: IntoIterator<Item = Update<'a>>,
    {
        updates
            .into_iter()
            .fold(Applied::default(), |mut ap, update| match update {
                Update::Direct { name, target, .. } => {
                    let name = name.into_qualified().into_owned();
                    let prev = match self.0.insert(name.clone(), target) {
                        Some(prev) => prev,
                        None => ObjectId::null(gix_hash::Kind::Sha1),
                    };
                    ap.updated.push(Updated::Direct {
                        name: name.to_ref_string(),
                        prev,
                        target,
                    });
                    ap
                }
                Update::Symbolic { name, target, .. } => {
                    let name = name.into_qualified().into_owned();
                    let prev = match self.0.insert(name.clone(), target.target) {
                        Some(prev) => prev,
                        None => ObjectId::null(gix_hash::Kind::Sha1),
                    };
                    ap.updated.push(Updated::Symbolic {
                        name: name.to_ref_string(),
                        prev,
                        target: target.name.to_ref_string(),
                    });
                    ap
                }
                Update::Prune { name, .. } => {
                    let name = name.into_qualified().into_owned();
                    if let Some((name, prev)) = self.0.remove_entry(&name) {
                        ap.updated.push(Updated::Prune {
                            name: name.to_ref_string(),
                            prev,
                        })
                    }
                    ap
                }
            })
    }
}

pub struct Scan<'a> {
    prefix: Option<String>,
    inner: hash_map::Iter<'a, Qualified<'static>, ObjectId>,
}

impl<'a> Iterator for Scan<'a> {
    type Item = Ref;

    fn next(&mut self) -> Option<Self::Item> {
        let (name, target) = self.inner.next()?;
        match &self.prefix {
            None => Some(Ref {
                name: name.to_owned(),
                target: Either::Left(*target),
                peeled: *target,
            }),
            Some(p) => {
                if name.starts_with(p) {
                    Some(Ref {
                        name: name.to_owned(),
                        target: Either::Left(*target),
                        peeled: *target,
                    })
                } else {
                    None
                }
            }
        }
    }
}
