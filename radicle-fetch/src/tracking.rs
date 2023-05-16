use std::collections::HashSet;

use radicle_crypto::PublicKey;

pub trait Tracking {
    type Error: std::error::Error + Send + Sync + 'static;

    fn tracked(&self) -> Result<HashSet<PublicKey>, Self::Error>;
}
