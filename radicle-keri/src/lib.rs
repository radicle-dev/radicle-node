pub enum KeyEvent {}

// TODO: seal the trait
pub trait Key {
    const IDENTIFIER: &'static str;
}

/// v: Version String
pub struct Version(String);

impl Key for Version {
    const IDENTIFIER: &'static str = "v";
}

/// i: Identifier Prefix
pub struct IdentifierPrefix(String);

impl Key for IdentifierPrefix {
    const IDENTIFIER: &'static str = "i";
}

/// s: Sequence Number
pub struct SequenceNumber(usize);

impl Key for SequenceNumber {
    const IDENTIFIER: &'static str = "s";
}

/// t: Message Type
pub struct MessageType;

impl Key for MessageType {
    const IDENTIFIER: &'static str = "t";
}

/// d: Event Digest (Seal or Receipt)
pub struct EventDigest;

impl Key for EventDigest {
    const IDENTIFIER: &'static str = "d";
}

/// p: Prior Event Digest
pub struct PriorEventDigest;

impl Key for PriorEventDigest {
    const IDENTIFIER: &'static str = "p";
}

/// kt: Keys Signing Threshold
pub struct KeysThreshold(usize);

impl Key for KeysThreshold {
    const IDENTIFIER: &'static str = "kt";
}

/// k: List of Signing Keys (ordered key set)
pub struct SigningKeys;

impl Key for SigningKeys {
    const IDENTIFIER: &'static str = "k";
}

/// n: Next Key Set Commitment
pub struct NextKeySetCommitment;

impl Key for NextKeySetCommitment {
    const IDENTIFIER: &'static str = "n";
}

/// wt: Witnessing Threshold
pub struct WitnessingThreshold;

impl Key for WitnessingThreshold {
    const IDENTIFIER: &'static str = "wt";
}

/// w: List of Witnesses (ordered witness set)
pub struct Witnesses;

impl Key for Witnesses {
    const IDENTIFIER: &'static str = "w";
}

/// wr: List of Witnesses to Remove (ordered witness set)
pub struct RemoveWitnesses;

impl Key for RemoveWitnesses {
    const IDENTIFIER: &'static str = "wr";
}

/// wa: List of Witnesses to Add (ordered witness set)
pub struct AddWitnesses;

impl Key for AddWitnesses {
    const IDENTIFIER: &'static str = "wa";
}

/// c: List of Configuration Traits/Modes
pub struct ConfigurationModes;

impl Key for ConfigurationModes {
    const IDENTIFIER: &'static str = "c";
}

/// a: List of Anchors (seals)
pub struct Anchors;

impl Key for Anchors {
    const IDENTIFIER: &'static str = "a";
}

/// da: Delegator Anchor Seal in Delegated Event (Location Seal)
pub struct DelegateAnchor;

impl Key for DelegateAnchor {
    const IDENTIFIER: &'static str = "da";
}

/// di: Delegator Identifier Prefix in Key State
pub struct DelegatorIdentifier;

impl Key for DelegatorIdentifier {
    const IDENTIFIER: &'static str = "di";
}

/// rd: Merkle Tree Root Digest
pub struct RootDigest;

impl Key for RootDigest {
    const IDENTIFIER: &'static str = "rd";
}

/// e: Last received Event Map in Key State
pub struct ReceivedEventMap;

impl Key for ReceivedEventMap {
    const IDENTIFIER: &'static str = "e";
}

/// ee: Last Establishment Event Map in Key State
pub struct EstablishedEventMap;

impl Key for EstablishedEventMap {
    const IDENTIFIER: &'static str = "ee";
}

/// vn: Version Number ("major.minor")
pub struct VersionNumber {
    major: String,
    minor: String,
}

impl Key for VersionNumber {
    const IDENTIFIER: &'static str = "vn";
}

pub enum Operation {
    Create,
    Read,
    Update,
    Deactivate,
}

pub trait Log {
    type ParentError: std::error::Error + Send + Sync + 'static;
    type GetError: std::error::Error + Send + Sync + 'static;

    type Id;
    type Entry;

    fn get(&self, id: Self::Id) -> Result<Self::Entry, Self::GetError>;

    fn parent(&self) -> Result<Self::Entry, Self::ParentError>;
}
