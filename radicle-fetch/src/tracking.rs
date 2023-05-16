use std::collections::HashSet;

use radicle_crypto::PublicKey;

#[derive(Clone, Debug)]
pub enum Scope {
    /// Fetch all available remotes.
    All,
    /// Fetch only trusted remotes.
    Trusted,
}

#[derive(Clone, Debug)]
pub struct Tracked {
    /// Whether the tracked scope wants to fetch all available remotes
    /// or only the trusted set of `tracked` remotes.
    pub scope: Scope,
    /// The set of `tracked` remotes.
    pub remotes: HashSet<PublicKey>,
}

pub trait Tracking {
    type Error: std::error::Error + Send + Sync + 'static;

    fn tracked(&self) -> Result<Tracked, Self::Error>;
}
