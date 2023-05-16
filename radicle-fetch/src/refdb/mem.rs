use std::collections::{hash_map, HashMap};
use std::convert::{self, Infallible};

use git::ref_format::Qualified;
use gix_hash::ObjectId;

use lib::refdb;
use lib::refdb::{Applied, RefScan, Refdb, Update, Updated};

#[derive(Default)]
pub struct InMemory {
    refs: HashMap<Qualified<'static>, ObjectId>,
}

impl FromIterator<(Qualified<'static>, ObjectId)> for InMemory {
    fn from_iter<T: IntoIterator<Item = (Qualified<'static>, ObjectId)>>(iter: T) -> Self {
        Self {
            refs: iter.into_iter().collect(),
        }
    }
}

impl From<HashMap<Qualified<'static>, ObjectId>> for InMemory {
    fn from(refs: HashMap<Qualified<'static>, ObjectId>) -> Self {
        Self { refs }
    }
}

impl Refdb for InMemory {
    type Oid = ObjectId;

    type FindError = Infallible;
    type TxError = Infallible;
    type ReloadError = Infallible;

    fn refname_to_id<'a, Q>(&self, refname: Q) -> Result<Option<Self::Oid>, Self::FindError>
    where
        Q: AsRef<Qualified<'a>>,
    {
        Ok(self.refs.get(refname.as_ref()).copied())
    }

    fn update<'a, I>(&mut self, updates: I) -> Result<Applied<'a>, Self::TxError>
    where
        I: IntoIterator<Item = Update<'a>>,
    {
        let mut ap = Applied::default();
        for update in updates.into_iter() {
            match update {
                Update::Direct {
                    name,
                    target,
                    no_ff: _,
                } => {
                    let name = name.into_owned();
                    self.refs.insert(name.clone(), target);
                    ap.updated.push(Updated::Direct {
                        name: name.into_refstring(),
                        target,
                    });
                }
                Update::Symbolic {
                    name,
                    target,
                    type_change,
                } => {
                    let name = name.into_owned();
                    self.refs.insert(name.clone(), target.target);
                    ap.updated.push(Updated::Symbolic {
                        name: name.into_refstring(),
                        target: target.name().to_owned(),
                    });
                }
                Update::Prune { name, prev } => {
                    let name = name.into_owned();
                    if let Some(_) = self.refs.remove(&name) {
                        ap.updated.push(Updated::Prune {
                            name: name.into_refstring(),
                        })
                    }
                }
            }
        }

        Ok(ap)
    }

    fn reload(&mut self) -> Result<(), Self::ReloadError> {
        Ok(())
    }
}

impl<'a> RefScan for &'a InMemory {
    type Oid = ObjectId;
    type Scan = Scan<'a, Self::Oid>;
    type Error = Infallible;

    fn scan<O, P>(self, prefix: O) -> Result<Self::Scan, Self::Error>
    where
        O: Into<Option<P>>,
        P: AsRef<str>,
    {
        let prefix = prefix.into();
        Ok(Scan {
            pref: prefix.map(|p| p.as_ref().to_owned()),
            iter: self.refs.iter(),
        })
    }
}

pub struct Scan<'a, Oid> {
    pref: Option<String>,
    iter: hash_map::Iter<'a, Qualified<'a>, Oid>,
}

impl<'a, Oid> Iterator for Scan<'a, Oid>
where
    Oid: Clone + 'a,
{
    type Item = Result<refdb::Ref<Oid>, Infallible>;

    fn next(&mut self) -> Option<Self::Item> {
        use either::Either::*;

        let next = self.iter.next().and_then(|(k, v)| match &self.pref {
            None => Some(refdb::Ref {
                name: k.to_owned(),
                target: Left(v.clone()),
                peeled: v.clone(),
            }),
            Some(p) => {
                if k.starts_with(p) {
                    Some(refdb::Ref {
                        name: k.to_owned(),
                        target: Left(v.clone()),
                        peeled: v.clone(),
                    })
                } else {
                    None
                }
            }
        });

        next.map(Ok)
    }
}
