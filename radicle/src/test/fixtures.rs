use std::path::Path;
use std::str::FromStr;

use crate::crypto::{PublicKey, Signer, Verified};
use crate::git;
use crate::identity::doc::Visibility;
use crate::identity::Id;
use crate::node::Alias;
use crate::rad;
use crate::storage::git::transport;
use crate::storage::git::Storage;
use crate::storage::refs::SignedRefs;

/// The birth of the radicle project, January 1st, 2018.
pub const RADICLE_EPOCH: i64 = 1514817556;

pub fn user() -> git::UserInfo {
    git::UserInfo {
        alias: Alias::new("Radcliff"),
        key: PublicKey::from_str("z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi").unwrap(),
    }
}

/// Create a new storage with a project.
pub fn storage<P: AsRef<Path>, G: Signer>(path: P, signer: &G) -> Result<Storage, rad::InitError> {
    let path = path.as_ref();
    let storage = Storage::open(path.join("storage"), user())?;

    transport::local::register(storage.clone());
    transport::remote::mock::register(signer.public_key(), storage.path());

    for (name, desc) in [
        ("acme", "Acme's repository"),
        ("vim", "A text editor"),
        ("rx", "A pixel editor"),
    ] {
        let (repo, _) = repository(path.join("workdir").join(name));
        rad::init(
            &repo,
            name,
            desc,
            git::refname!("master"),
            Visibility::default(),
            signer,
            &storage,
        )?;
    }

    Ok(storage)
}

/// Create a new repository at the given path, and initialize it into a project.
pub fn project<P: AsRef<Path>, G: Signer>(
    path: P,
    storage: &Storage,
    signer: &G,
) -> Result<(Id, SignedRefs<Verified>, git2::Repository, git2::Oid), rad::InitError> {
    transport::local::register(storage.clone());

    let (working, head) = repository(path);
    let (id, _, refs) = rad::init(
        &working,
        "acme",
        "Acme's repository",
        git::refname!("master"),
        Visibility::default(),
        signer,
        storage,
    )?;

    Ok((id, refs, working, head))
}

/// Creates a regular repository at the given path with a couple of commits.
pub fn repository<P: AsRef<Path>>(path: P) -> (git2::Repository, git2::Oid) {
    let repo = git2::Repository::init(path).unwrap();
    let sig = git2::Signature::new(
        "anonymous",
        "anonymous@radicle.xyz",
        &git2::Time::new(RADICLE_EPOCH, 0),
    )
    .unwrap();
    let head = git::initial_commit(&repo, &sig).unwrap();
    let tree = git::write_tree(Path::new("README"), "Hello World!\n".as_bytes(), &repo).unwrap();
    let oid = {
        let commit = git::commit(
            &repo,
            &head,
            git::refname!("refs/heads/master").as_refstr(),
            "Second commit",
            &sig,
            &tree,
        )
        .unwrap();

        commit.id()
    };
    repo.set_head("refs/heads/master").unwrap();
    repo.checkout_head(None).unwrap();

    drop(tree);
    drop(head);

    (repo, oid)
}

/// Create an empty commit on the current branch.
pub fn commit(msg: &str, parents: &[git2::Oid], repo: &git2::Repository) -> git::Oid {
    let head = repo.head().unwrap();
    let sig = git2::Signature::new(
        "anonymous",
        "anonymous@radicle.xyz",
        &git2::Time::new(RADICLE_EPOCH, 0),
    )
    .unwrap();
    let tree = head.peel_to_commit().unwrap().tree().unwrap();
    let parents = parents
        .iter()
        .map(|p| repo.find_commit(*p).unwrap())
        .collect::<Vec<_>>();
    let parents = parents.iter().collect::<Vec<_>>(); // Get references.

    repo.commit(None, &sig, &sig, msg, &tree, &parents)
        .unwrap()
        .into()
}

/// Populate a repository with commits, branches and blobs.
pub fn populate(repo: &git2::Repository, scale: usize) -> Vec<git::Qualified> {
    assert!(
        scale <= 8,
        "Scale parameter must be less than or equal to 8"
    );
    if scale == 0 {
        return vec![];
    }
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    let mut rng = fastrand::Rng::with_seed(42);
    let mut buffer = vec![0; 1024 * 1024 * scale];
    let mut refs = Vec::new();

    for _ in 0..scale {
        let random = std::iter::repeat_with(|| rng.alphanumeric())
            .take(7)
            .collect::<String>()
            .to_lowercase();
        let name =
            git::refname!("feature").join(git::RefString::try_from(random.as_str()).unwrap());
        let signature = git2::Signature::now("Radicle", "radicle@radicle.xyz").unwrap();

        rng.fill(&mut buffer);

        let blob = repo.blob(&buffer).unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("random.txt", blob, 0o100_644).unwrap();
        let tree_oid = builder.write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let refstr = git::refs::workdir::branch(&name);

        repo.commit(
            Some(&refstr),
            &signature,
            &signature,
            &format!("Initialize new branch {name}"),
            &tree,
            &[&head],
        )
        .unwrap();

        refs.push(git::Qualified::from_refstr(refstr).unwrap());
    }
    refs
}

/// Generate random fixtures.
pub mod gen {
    use super::*;

    /// Generate a random string of the given length.
    pub fn string(length: usize) -> String {
        std::iter::repeat_with(fastrand::alphabetic)
            .take(length)
            .collect::<String>()
    }

    /// Generate a random email.
    pub fn email() -> String {
        format!("{}@{}.xyz", string(6), string(6))
    }

    /// Creates a regular repository at the given path with a couple of commits.
    pub fn repository<P: AsRef<Path>>(path: P) -> (git2::Repository, git2::Oid) {
        let repo = git2::Repository::init(path).unwrap();
        let sig = git2::Signature::now(string(6).as_str(), email().as_str()).unwrap();
        let head = git::initial_commit(&repo, &sig).unwrap();
        let tree =
            git::write_tree(Path::new("README"), "Hello World!\n".as_bytes(), &repo).unwrap();
        let oid = git::commit(
            &repo,
            &head,
            git::refname!("refs/heads/master").as_refstr(),
            string(16).as_str(),
            &sig,
            &tree,
        )
        .unwrap()
        .id();

        // Look, I don't really understand why we have to do this, but we do.
        drop(head);
        drop(tree);

        (repo, oid)
    }
}
