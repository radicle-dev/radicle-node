#![allow(clippy::unwrap_used)]
pub mod arbitrary;
pub mod assert;
pub mod fixtures;
pub mod storage;

use super::storage::{Namespaces, RefUpdate};

use crate::prelude::NodeId;
use crate::storage::WriteRepository;

/// Perform a fetch between two local repositories.
/// This has the same outcome as doing a "real" fetch, but suffices for the simulation, and
/// doesn't require running nodes.
pub fn fetch<W: WriteRepository>(
    repo: &W,
    node: &NodeId,
    namespaces: impl Into<Namespaces>,
) -> Result<Vec<RefUpdate>, crate::storage::FetchError> {
    let namespace = match namespaces.into() {
        Namespaces::All => None,
        Namespaces::Trusted(trusted) => trusted.into_iter().next(),
    };
    let mut updates = Vec::new();
    let mut callbacks = git2::RemoteCallbacks::new();
    let mut opts = git2::FetchOptions::default();
    let refspec = if let Some(namespace) = namespace {
        opts.prune(git2::FetchPrune::On);
        format!("refs/namespaces/{namespace}/refs/*:refs/namespaces/{namespace}/refs/*")
    } else {
        opts.prune(git2::FetchPrune::Off);
        "refs/namespaces/*:refs/namespaces/*".to_owned()
    };

    callbacks.update_tips(|name, old, new| {
        if let Ok(name) = crate::git::RefString::try_from(name) {
            if name.to_namespaced().is_some() {
                updates.push(RefUpdate::from(name, old, new));
                // Returning `true` ensures the process is not aborted.
                return true;
            }
        }
        false
    });
    opts.remote_callbacks(callbacks);

    let mut remote = repo.raw().remote_anonymous(
        crate::storage::git::transport::remote::Url {
            node: *node,
            repo: repo.id(),
            namespace,
        }
        .to_string()
        .as_str(),
    )?;
    remote.fetch(&[refspec], Some(&mut opts), None)?;

    drop(opts);

    repo.set_identity_head()?;
    repo.set_head()?;
    let validations = repo.validate()?;
    if !validations.is_empty() {
        return Err(crate::storage::FetchError::Validation { validations });
    }

    Ok(updates)
}

pub mod setup {
    use std::path::{Path, PathBuf};

    use super::storage::{Namespaces, RefUpdate};
    use crate::crypto::test::signer::MockSigner;
    use crate::storage::git::transport::remote;
    use crate::{
        git,
        profile::Home,
        rad::REMOTE_NAME,
        test::{fixtures, storage::git::Repository},
        Storage,
    };
    use crate::{prelude::*, rad};

    /// A node.
    ///
    /// Note that this isn't a real node; only a profile with storage and a signing key.
    pub struct Node {
        pub root: PathBuf,
        pub storage: Storage,
        pub signer: MockSigner,
    }

    impl Default for Node {
        fn default() -> Self {
            let root = tempfile::tempdir().unwrap();

            Self::new(root, MockSigner::default(), "Radcliff")
        }
    }

    impl Node {
        pub fn new(root: impl AsRef<Path>, signer: MockSigner, alias: &str) -> Self {
            let root = root.as_ref().to_path_buf();
            let home = root.join("home");
            let paths = Home::new(home.as_path()).unwrap();
            let storage = Storage::open(
                paths.storage(),
                git::UserInfo {
                    alias: Alias::new(alias),
                    key: *signer.public_key(),
                },
            )
            .unwrap();

            remote::mock::register(signer.public_key(), storage.path());

            Self {
                root,
                storage,
                signer,
            }
        }

        pub fn clone(&mut self, rid: Id, other: &Self) {
            let repo = self.storage.create(rid).unwrap();
            super::fetch(&repo, other.signer.public_key(), Namespaces::All).unwrap();

            rad::fork(rid, &self.signer, &self.storage).unwrap();
        }

        pub fn project(&self) -> NodeRepo {
            let (id, _, checkout, _) =
                fixtures::project(self.root.join("working"), &self.storage, &self.signer).unwrap();
            let repo = self.storage.repository(id).unwrap();
            let checkout = Some(NodeRepoCheckout { checkout });

            NodeRepo { repo, checkout }
        }
    }

    /// A node repository with an optional checkout.
    pub struct NodeRepo {
        pub repo: Repository,
        pub checkout: Option<NodeRepoCheckout>,
    }

    impl NodeRepo {
        #[track_caller]
        pub fn fetch(&self, from: &Node) -> Vec<RefUpdate> {
            super::fetch(&self.repo, from.signer.public_key(), Namespaces::All).unwrap()
        }

        #[track_caller]
        pub fn checkout(&self) -> &NodeRepoCheckout {
            self.checkout.as_ref().unwrap()
        }
    }

    impl std::ops::Deref for NodeRepo {
        type Target = Repository;

        fn deref(&self) -> &Self::Target {
            &self.repo
        }
    }

    impl std::ops::DerefMut for NodeRepo {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.repo
        }
    }

    /// A repository checkout.
    pub struct NodeRepoCheckout {
        checkout: git::raw::Repository,
    }

    impl NodeRepoCheckout {
        pub fn branch_with<S: AsRef<Path>, T: AsRef<[u8]>>(
            &self,
            blobs: impl IntoIterator<Item = (S, T)>,
        ) -> BranchWith {
            let refname = git::Qualified::from(git::lit::refs_heads(git::refname!("master")));
            let base = self.checkout.refname_to_id(refname.as_str()).unwrap();
            let parent = self.checkout.find_commit(base).unwrap();
            let oid = commit(&self.checkout, &refname, blobs, &[&parent]);

            git::push(&self.checkout, &REMOTE_NAME, [(&refname, &refname)]).unwrap();

            BranchWith {
                base: base.into(),
                oid,
            }
        }
    }

    impl std::ops::Deref for NodeRepoCheckout {
        type Target = git::raw::Repository;

        fn deref(&self) -> &Self::Target {
            &self.checkout
        }
    }

    /// A node with a repository.
    pub struct NodeWithRepo {
        pub node: Node,
        pub repo: NodeRepo,
    }

    impl std::ops::Deref for NodeWithRepo {
        type Target = Node;

        fn deref(&self) -> &Self::Target {
            &self.node
        }
    }

    impl std::ops::DerefMut for NodeWithRepo {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.node
        }
    }

    impl Default for NodeWithRepo {
        fn default() -> Self {
            let node = Node::default();
            let repo = node.project();

            Self { node, repo }
        }
    }

    /// A network of three nodes.
    ///
    /// Note that these are not actually running nodes in the sense of `radicle-node`.
    /// These are simply profiles with their own storage, and the ability to fetch between
    /// them.
    pub struct Network {
        pub alice: NodeWithRepo,
        pub bob: NodeWithRepo,
        pub eve: NodeWithRepo,
        pub rid: Id,

        #[allow(dead_code)]
        tmp: tempfile::TempDir,
    }

    impl Default for Network {
        fn default() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let alice = Node::new(
                tmp.path().join("alice"),
                MockSigner::from_seed([!0; 32]),
                "alice",
            );
            let mut bob = Node::new(
                tmp.path().join("bob"),
                MockSigner::from_seed([!1; 32]),
                "bob",
            );
            let mut eve = Node::new(
                tmp.path().join("eve"),
                MockSigner::from_seed([!2; 32]),
                "eve",
            );
            let repo = alice.project();
            let rid = repo.id;

            bob.clone(repo.id, &alice);
            eve.clone(repo.id, &alice);

            let alice = NodeWithRepo { node: alice, repo };
            let repo = bob.storage.repository(rid).unwrap();
            let bob = NodeWithRepo {
                node: bob,
                repo: NodeRepo {
                    repo,
                    checkout: None,
                },
            };
            let repo = eve.storage.repository(rid).unwrap();
            let eve = NodeWithRepo {
                node: eve,
                repo: NodeRepo {
                    repo,
                    checkout: None,
                },
            };

            Self {
                alice,
                bob,
                eve,
                rid,
                tmp,
            }
        }
    }

    #[derive(Debug)]
    pub struct BranchWith {
        pub base: git::Oid,
        pub oid: git::Oid,
    }

    pub fn commit<S: AsRef<Path>, T: AsRef<[u8]>>(
        repo: &git2::Repository,
        refname: &git::Qualified,
        blobs: impl IntoIterator<Item = (S, T)>,
        parents: &[&git2::Commit<'_>],
    ) -> git::Oid {
        let tree = {
            let mut tb = repo.treebuilder(None).unwrap();
            for (name, blob) in blobs.into_iter() {
                let oid = repo.blob(blob.as_ref()).unwrap();
                tb.insert(name.as_ref(), oid, git2::FileMode::Blob.into())
                    .unwrap();
            }
            tb.write().unwrap()
        };
        let tree = repo.find_tree(tree).unwrap();
        let author = git2::Signature::now("anonymous", "anonymous@example.com").unwrap();

        repo.commit(
            Some(refname.as_str()),
            &author,
            &author,
            "Making changes",
            &tree,
            parents,
        )
        .unwrap()
        .into()
    }
}
