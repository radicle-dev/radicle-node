use nonempty::NonEmpty;
use radicle_crypto::PublicKey;

pub trait Verified {
    type Id;

    fn delegates(&self) -> NonEmpty<PublicKey>;
    fn id(&self) -> Self::Id;
}

pub trait Identities {
    type VerifiedIdentity: Verified;

    type VerifiedError;

    fn verified(&self, head: ObjectId) -> Result<Self::VerifiedIdentity, Self::VerifiedError>;
}

pub fn current<C>(cx: &C) -> Result<Option<C::VerifiedIdentity>, Error>
where
    C: Identities + Refdb,
{
    Ok(
        Refdb::refname_to_id(cx, refs::Qualified::from(refs::REFS_RAD_ID))?
            .map(|tip| Identities::verify(cx, tip))
            .transpose()?,
    )
}
