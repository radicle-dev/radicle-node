pub mod odb;
pub use odb::Odb;

pub mod refdb;
pub use refdb::Refdb;

pub use bstr::BString;
pub use gix_hash::ObjectId;

pub mod oid {
    //! Helper functions for converting to/from [`git_ext::Oid`] and
    //! [`ObjectId`].

    use super::ObjectId;
    use git_ext::Oid;

    /// Convert from an [`ObjectId`] to an [`Oid`].
    pub fn to_oid(oid: ObjectId) -> Oid {
        Oid::try_from(oid.as_bytes()).expect("invalid gix Oid")
    }

    /// Convert from an [`Oid`] to an [`ObjectId`].
    pub fn to_object_id(oid: Oid) -> ObjectId {
        ObjectId::try_from(oid.as_bytes()).expect("invalid git-ext Oid")
    }
}
