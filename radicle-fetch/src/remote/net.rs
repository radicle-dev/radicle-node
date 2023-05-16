use std::io;
use std::path::PathBuf;

use gix_hash::ObjectId;
use gix_object::bstr::BString;
use gix_transport::client;
use nonempty::nonzero::NonEmpty;

use crate::git::{odb::Odb, refdb::Refdb};
use crate::refdb::Ref;
use crate::transport::{self, fetch, ls_refs};

// TODO: consider making C concrete
pub struct Network<Id, C> {
    storage: PathBuf,
    rid: Id,
    odb: Odb,
    refdb: Refdb,
    conn: C,
}

impl<Id, C> Network<Id, C>
where
    C: client::Transport,
{
    fn ls_refs(&self, mut prefixes: Vec<BString>) -> Result<Vec<Ref>, transport::ls_refs::Error> {
        prefixes.sort();
        prefixes.dedup();
        ls_refs(transport::ls_refs::Config { prefixes }, self.conn)
    }

    fn fetch(
        &self,
        wants: NonEmpty<ObjectId>,
        haves: Vec<ObjectId>,
    ) -> Result<(), transport::fetch::Error> {
        let wants = Vec::from(wants);
        let out = {
            // FIXME: make options work with slice
            let wants = wants.clone();
            let thick: B::Owned = self.db.as_ref().to_owned();
            fetch(
                transport::fetch::Fetch {
                    config: transport::fetch::Config { wants, haves },
                    refs: todo!(),
                    pack: todo!(),
                },
                self.conn,
            )?;
        };
        let pack_path = out
            .pack
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "empty or no packfile received",
                )
            })?
            .index_path
            .expect("written packfile must have a path");

        // Validate we got all requested tips in the pack
        {
            use gix_pack::index::File;

            let idx = File::at(&pack_path, gix_hash::Kind::Sha1).map_err(io_other)?;
            for oid in wants {
                if idx.lookup(oid).is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("wanted {} not found in pack", oid),
                    ));
                }
            }
        }
        // abstraction leak: we could add the `Index` directly if we knew the
        // type of our odb.
        self.db.add_pack(&pack_path).map_err(io_other)?;

        Ok(())
    }
}
