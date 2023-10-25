//! Radicle node profile.
//!
//!   $RAD_HOME/                                 # Radicle home
//!     storage/                                 # Storage root
//!       zEQNunJUqkNahQ8VvQYuWZZV7EJB/          # Project git repository
//!       ...                                    # More projects...
//!     keys/
//!       radicle                                # Secret key (PKCS 8)
//!       radicle.pub                            # Public key (PKCS 8)
//!     node/
//!       control.sock                           # Node control socket
//!
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fs, io, str::FromStr};

use serde::Serialize;
use thiserror::Error;

use crate::crypto::ssh::agent::Agent;
use crate::crypto::ssh::{keystore, Keystore, Passphrase};
use crate::crypto::{PublicKey, Signer};
use crate::node::{address, routing, tracking, Alias, AliasStore};
use crate::prelude::Did;
use crate::prelude::NodeId;
use crate::storage::git::transport;
use crate::storage::git::Storage;
use crate::{git, node};

/// Environment variables used by radicle.
pub mod env {
    pub use std::env::*;

    /// Path to the radicle home folder.
    pub const RAD_HOME: &str = "RAD_HOME";
    /// Path to the radicle node socket file.
    pub const RAD_SOCKET: &str = "RAD_SOCKET";
    /// Passphrase for the encrypted radicle secret key.
    pub const RAD_PASSPHRASE: &str = "RAD_PASSPHRASE";
    /// RNG seed. Must be convertible to a `u64`.
    pub const RAD_RNG_SEED: &str = "RAD_RNG_SEED";

    /// Get the configured pager program from the environment.
    pub fn pager() -> Option<String> {
        if let Ok(cfg) = git2::Config::open_default() {
            if let Ok(pager) = cfg.get_string("core.pager") {
                return Some(pager);
            }
        }
        if let Ok(pager) = var("PAGER") {
            return Some(pager);
        }
        None
    }

    /// Get the radicle passphrase from the environment.
    pub fn passphrase() -> Option<super::Passphrase> {
        let Ok(passphrase) = var(RAD_PASSPHRASE) else {
            return None;
        };
        Some(super::Passphrase::from(passphrase))
    }

    /// Get a random number generator from the environment.
    pub fn rng() -> fastrand::Rng {
        if let Ok(seed) = var(RAD_RNG_SEED) {
            return fastrand::Rng::with_seed(
                seed.parse()
                    .expect("env::rng: invalid seed specified in `RAD_RNG_SEED`"),
            );
        }
        fastrand::Rng::new()
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Keystore(#[from] keystore::Error),
    #[error(transparent)]
    MemorySigner(#[from] keystore::MemorySignerError),
    #[error("no profile found at path '{0}'")]
    NotFound(PathBuf),
    #[error("error connecting to ssh-agent: {0}")]
    Agent(#[from] crate::crypto::ssh::agent::Error),
    #[error("profile key `{0}` is not registered with ssh-agent")]
    KeyNotRegistered(PublicKey),
    #[error(transparent)]
    TrackingStore(#[from] node::tracking::store::Error),
    #[error(transparent)]
    AddressStore(#[from] node::address::Error),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to load node configuration from {0}: {1}")]
    Io(PathBuf, io::Error),
    #[error("failed to decode node configuration from {0}: {1}")]
    Json(PathBuf, serde_json::Error),
}

/// Local radicle configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub node: node::Config,
}

impl Config {
    /// Initialize a new configuration. Fails if the path already exists.
    pub fn init(alias: Alias, path: &Path) -> io::Result<Self> {
        let cfg = Self {
            node: node::Config::new(alias),
        };
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"  ");
        let mut serializer = serde_json::Serializer::with_formatter(&file, formatter);

        cfg.serialize(&mut serializer)?;
        file.write_all(b"\n")?;
        file.sync_all()?;

        Ok(cfg)
    }

    /// Load a configuration from the given path.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::File::open(path) {
            Ok(cfg) => {
                serde_json::from_reader(cfg).map_err(|e| ConfigError::Json(path.to_path_buf(), e))
            }
            Err(e) => {
                let Ok(user) = env::var("USER") else {
                    return Err(ConfigError::Io(path.to_owned(), e));
                };
                let Ok(alias) = Alias::from_str(&user) else {
                    return Err(ConfigError::Io(path.to_owned(), e));
                };
                Ok(Config {
                    node: node::Config::new(alias),
                })
            }
        }
    }

    /// Get the user alias.
    pub fn alias(&self) -> &Alias {
        &self.node.alias
    }
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub home: Home,
    pub storage: Storage,
    pub keystore: Keystore,
    pub public_key: PublicKey,
    pub config: Config,
}

impl Profile {
    pub fn init(home: Home, alias: Alias, passphrase: Option<Passphrase>) -> Result<Self, Error> {
        let keystore = Keystore::new(&home.keys());
        let public_key = keystore.init("radicle", passphrase)?;
        let config = Config::init(alias.clone(), home.config().as_path())?;
        let storage = Storage::open(
            home.storage(),
            git::UserInfo {
                alias,
                key: public_key,
            },
        )?;

        transport::local::register(storage.clone());

        Ok(Profile {
            home,
            storage,
            keystore,
            public_key,
            config,
        })
    }

    pub fn load() -> Result<Self, Error> {
        let home = self::home()?;
        let keystore = Keystore::new(&home.keys());
        let public_key = keystore
            .public_key()?
            .ok_or_else(|| Error::NotFound(home.path().to_path_buf()))?;
        let config = Config::load(home.config().as_path())?;
        let storage = Storage::open(
            home.storage(),
            git::UserInfo {
                alias: config.alias().clone(),
                key: public_key,
            },
        )?;

        transport::local::register(storage.clone());

        Ok(Profile {
            home,
            storage,
            keystore,
            public_key,
            config,
        })
    }

    pub fn id(&self) -> &PublicKey {
        &self.public_key
    }

    pub fn info(&self) -> git::UserInfo {
        git::UserInfo {
            alias: self.config.alias().clone(),
            key: *self.id(),
        }
    }

    pub fn did(&self) -> Did {
        Did::from(self.public_key)
    }

    pub fn signer(&self) -> Result<Box<dyn Signer>, Error> {
        if !self.keystore.is_encrypted()? {
            let signer = keystore::MemorySigner::load(&self.keystore, None)?;
            return Ok(signer.boxed());
        }

        if let Some(passphrase) = env::passphrase() {
            let signer = keystore::MemorySigner::load(&self.keystore, Some(passphrase))?;
            return Ok(signer.boxed());
        }

        match Agent::connect() {
            Ok(agent) => {
                let signer = agent.signer(self.public_key);
                if signer.is_ready()? {
                    Ok(signer.boxed())
                } else {
                    Err(Error::KeyNotRegistered(self.public_key))
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Return a read-only handle to the tracking configuration of the node.
    pub fn tracking(&self) -> Result<tracking::store::ConfigReader, tracking::store::Error> {
        let path = self.home.node().join(node::TRACKING_DB_FILE);
        let config = tracking::store::Config::reader(path)?;

        Ok(config)
    }

    /// Return a read-write handle to the tracking configuration of the node.
    pub fn tracking_mut(&self) -> Result<tracking::store::ConfigWriter, tracking::store::Error> {
        let path = self.home.node().join(node::TRACKING_DB_FILE);
        let config = tracking::store::Config::open(path)?;

        Ok(config)
    }

    /// Return a read-only handle to the routing database of the node.
    pub fn routing(&self) -> Result<routing::Table, routing::Error> {
        let path = self.home.node().join(node::ROUTING_DB_FILE);
        let router = routing::Table::reader(path)?;

        Ok(router)
    }

    /// Return a handle to the addresses database of the node.
    pub fn addresses(&self) -> Result<address::Book, address::Error> {
        let path = self.home.node().join(node::ADDRESS_DB_FILE);
        let addresses = address::Book::reader(path)?;

        Ok(addresses)
    }

    /// Get radicle home.
    pub fn home(&self) -> &Home {
        &self.home
    }

    /// Return a multi-source store for aliases.
    pub fn aliases(&self) -> Aliases {
        let tracking = self.home.tracking().ok();
        let addresses = self.home.addresses().ok();

        Aliases {
            tracking,
            addresses,
        }
    }
}

impl std::ops::Deref for Profile {
    type Target = Home;

    fn deref(&self) -> &Self::Target {
        &self.home
    }
}

impl std::ops::DerefMut for Profile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.home
    }
}

/// Holds multiple alias stores, and will try
/// them one by one when asking for an alias.
pub struct Aliases {
    tracking: Option<tracking::store::ConfigReader>,
    addresses: Option<address::Book>,
}

impl AliasStore for Aliases {
    /// Retrieve `alias` of given node.
    /// First looks in `tracking.db` and then `addresses.db`.
    fn alias(&self, nid: &NodeId) -> Option<Alias> {
        self.tracking
            .as_ref()
            .and_then(|db| db.alias(nid))
            .or_else(|| self.addresses.as_ref().and_then(|db| db.alias(nid)))
    }
}

/// Get the path to the radicle home folder.
pub fn home() -> Result<Home, io::Error> {
    if let Some(home) = env::var_os(env::RAD_HOME) {
        Ok(Home::new(PathBuf::from(home))?)
    } else if let Some(home) = env::var_os("HOME") {
        Ok(Home::new(PathBuf::from(home).join(".radicle"))?)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Neither `RAD_HOME` nor `HOME` are set",
        ))
    }
}

/// Radicle home.
#[derive(Debug, Clone)]
pub struct Home {
    path: PathBuf,
}

impl TryFrom<PathBuf> for Home {
    type Error = io::Error;

    fn try_from(home: PathBuf) -> Result<Self, Self::Error> {
        Self::new(home)
    }
}

impl Home {
    /// Creates the Radicle Home directories.
    ///
    /// The `home` path is used as the base directory for all
    /// necessary subdirectories.
    ///
    /// If `home` does not already exist then it and any
    /// subdirectories are created using [`fs::create_dir_all`].
    ///
    /// The `home` path is also canonicalized using [`fs::canonicalize`].
    ///
    /// All necessary subdirectories are also created.
    pub fn new(home: impl Into<PathBuf>) -> Result<Self, io::Error> {
        let path = home.into();
        if !path.exists() {
            fs::create_dir_all(path.clone())?;
        }
        let home = Self {
            path: path.canonicalize()?,
        };

        for dir in &[home.storage(), home.keys(), home.node()] {
            if !dir.exists() {
                fs::create_dir_all(dir)?;
            }
        }

        Ok(home)
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn storage(&self) -> PathBuf {
        self.path.join("storage")
    }

    pub fn config(&self) -> PathBuf {
        self.path.join("config.json")
    }

    pub fn keys(&self) -> PathBuf {
        self.path.join("keys")
    }

    pub fn node(&self) -> PathBuf {
        self.path.join("node")
    }

    pub fn socket(&self) -> PathBuf {
        env::var_os(env::RAD_SOCKET)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.node().join(node::DEFAULT_SOCKET_NAME))
    }

    /// Return a read-only handle to the tracking configuration of the node.
    pub fn tracking(&self) -> Result<tracking::store::ConfigReader, tracking::store::Error> {
        let path = self.node().join(node::TRACKING_DB_FILE);
        let config = tracking::store::Config::reader(path)?;

        Ok(config)
    }

    /// Return a read-write handle to the tracking configuration of the node.
    pub fn tracking_mut(&self) -> Result<tracking::store::ConfigWriter, tracking::store::Error> {
        let path = self.node().join(node::TRACKING_DB_FILE);
        let config = tracking::store::Config::open(path)?;

        Ok(config)
    }

    /// Return a read-only handle to the routing database of the node.
    pub fn routing(&self) -> Result<routing::Table, routing::Error> {
        let path = self.node().join(node::ROUTING_DB_FILE);
        let router = routing::Table::reader(path)?;

        Ok(router)
    }

    /// Return a read-write handle to the routing database of the node.
    pub fn routing_mut(&self) -> Result<routing::Table, routing::Error> {
        let path = self.node().join(node::ROUTING_DB_FILE);
        let router = routing::Table::open(path)?;

        Ok(router)
    }

    /// Return a handle to a read-only addresses database of the node.
    pub fn addresses(&self) -> Result<address::Book, address::Error> {
        let path = self.node().join(node::ADDRESS_DB_FILE);
        let addresses = address::Book::reader(path)?;

        Ok(addresses)
    }

    /// Return a handle to the addresses database of the node.
    pub fn addresses_mut(&self) -> Result<address::Book, address::Error> {
        let path = self.node().join(node::ADDRESS_DB_FILE);
        let addresses = address::Book::open(path)?;

        Ok(addresses)
    }
}

#[cfg(test)]
#[cfg(not(target_os = "macos"))]
mod test {
    use std::fs;

    use super::Home;

    // Checks that if we have:
    // '/run/user/1000/.tmpqfK6ih/../.tmpqfK6ih/Radicle/Home'
    //
    // that it gets normalized to:
    // '/run/user/1000/.tmpqfK6ih/Radicle/Home'
    #[test]
    fn canonicalize_home() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Home").join("Radicle");
        fs::create_dir_all(path.clone()).unwrap();

        let last = tmp.path().components().last().unwrap();
        let home = Home::new(
            tmp.path()
                .join("..")
                .join(last)
                .join("Home")
                .join("Radicle"),
        )
        .unwrap();

        assert_eq!(home.path, path);
    }
}
