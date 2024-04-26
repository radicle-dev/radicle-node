# HACKING

Welcome to the Radicle "Heartwood" hacking guide!

We appreciate your interest in contributing to the Radicle project. If you come across
a bug or a missing feature, please feel free to submit a patch. This guide is meant as
an introduction to the codebase, on how to debug issues, write tests and navigate the
repository.

Please make sure to read [CONTRIBUTING.md](CONTRIBUTING.md) before submitting code,
and follow the included guidelines. To download a development version of Radicle,
see the [README.md](README.md).

For an architectural overview of Heartwood, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

The repository is structured in *crates*, as follows:

* `radicle`: The Radicle standard library that contains shared libraries used across the project.
* `radicle-cli`: the Radicle command-line interface (`rad`).
* `radicle-cli-test`: The Radicle CLI testing framework, for writing documentation tests.
* `radicle-cob`: Radicle Collaborative Objects (COBs). Provides a way of creating and traversing edit histories.
* `radicle-crdt`: Conflict-free replicated datatypes (CRDTs) used for things like discussions and patches.
* `radicle-crypto`: A wrapper around Ed25519 cryptographic signing primitives.
* `radicle-dag`: A simple directed acyclic graph implementation used by `radicle-cob`.
* `radicle-httpd`: The radicle HTTP daemon that serves API clients and Git fetch requests.
* `radicle-node`: The radicle peer-to-peer daemon that enables users to connect to the network and share code.
* `radicle-remote-helper`: A Git remote helper for `rad://` remotes.
* `radicle-ssh`: OpenSSH functionality, including a library used to interface with `ssh-agent`.
* `radicle-term`: A generic terminal library used by the Radicle CLI.
* `radicle-tools`: Tools used to aid in the development of Radicle.

## Running in debug mode

To run the services or the CLI in debug mode, use `cargo run -p <package>`.

For example, the equivalent of `rad auth` in debug mode would be:

    $ cargo run -p radicle-cli --bin rad -- auth

Arguments after the `--` are passed directly to the `rad` executable.

When running the radicle node, you may specify an alternate port for the `git-daemon`
like so:

    $ cargo run -p radicle-node -- --git-daemon 127.0.0.1:9876

This is useful if you are running multiple nodes on the same machine. You can also
specify different listen addresses for the peer-to-peer protocol using `--listen`.
To view all options, run `cargo run -p radicle-node -- --help`.

You may want to set the appropriate environment variables before running these commands
to prevent them from interfering with an existing installation of radicle. See the
following section on environment variables.

## Environment variables

When developing radicle, some environment variables may be used to make the
development environment more friendly.

**`RAD_HOME`**

Set this to a path on your file system where you'd like radicle to store keys
and repositories. Typically you'll want to set this to a temporary folder, eg.
`/tmp/radicle`, that can be safely deleted. If set, all radicle data will be
stored within this folder.

**`RAD_SEED`**

Set this to a 32-byte hexadecimal string to generate deterministic Node IDs
when creating new profiles. For example, integration tests use the following
setting:

    RAD_SEED=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff

**`RAD_PASSPHRASE`**

Set this to the passphrase chosen during profile initialization (`rad auth`) to
skip the passphrase prompt. It's recommended to set this while developing to
avoid storing development keys with `ssh-agent`.

## Logging

Logging for `radicle-node` and `radicle-httpd` is turned on by default. Check
the respective `--help` output to set the log level.

## Writing tests

### Documentation tests

When implementing changes to the CLI, or adding a new sub-command, it's a good
idea to add a documentation test. You can find examples of these in
`radicle-cli/examples`.

Each documentation test must be accompanied by a regular unit test. These are
located in `radicle-cli/tests/commands.rs`. To keep tests deterministic,
environment variables are used. If your document test output is changing on
each test run, make sure to account for any variability in the test environment
(clocks, RNGs, etc.).

### Node service logic tests

When testing the core service logic, eg. the gossip protocol; tests can be
added to `radicle-node/src/tests.rs`. These service-level tests simply test
inputs and outputs and do not perform any I/O.

### Node end-to-end tests

If you find the need to test the replication protocol or networking layer, it's
possible to write an end-to-end test. These tests can be found in
`radicle-node/src/tests/e2e.rs`.

## Debugging

### Repository storage

Radicle stores git repositories inside `$RAD_HOME/storage`, which defaults to
`~/.radicle/storage` on UNIX-based operating systems. You can use standard git
tooling to inspect references and other git objects inside storage. Each radicle
repository is stored under its own folder under storage as a bare Git repository.

Once inside a repository folder, the following commands may come in handy.

`git show-ref` to show all references:

    $ git show-ref
    f60b291752bc38be7dfc90c4c4034de13e01a66b refs/heads/master
    f60b291752bc38be7dfc90c4c4034de13e01a66b refs/namespaces/z6MkqTY5aQepDGNCrkPqzdmzveX3D4oAmyVXUDDVQaDGdyVH/refs/heads/master
    805b7d0df927dcbc4d3911ab07cd497953eecbd1 refs/namespaces/z6MkqTY5aQepDGNCrkPqzdmzveX3D4oAmyVXUDDVQaDGdyVH/refs/rad/id
    86136a42a69572015466bac2d974154ee76f0853 refs/namespaces/z6MkqTY5aQepDGNCrkPqzdmzveX3D4oAmyVXUDDVQaDGdyVH/refs/rad/sigrefs
    5575035d8b4faf1f18c532b08516f18031dd7b28 refs/namespaces/z6MkuGSynjxM8SLhcsiEPWZgDeGLAVNXf5g7WePmc1Tri1FS/refs/heads/master
    805b7d0df927dcbc4d3911ab07cd497953eecbd1 refs/namespaces/z6MkuGSynjxM8SLhcsiEPWZgDeGLAVNXf5g7WePmc1Tri1FS/refs/rad/id
    89e4f0baa327595c7b2849189fc8808388a29033 refs/namespaces/z6MkuGSynjxM8SLhcsiEPWZgDeGLAVNXf5g7WePmc1Tri1FS/refs/rad/sigrefs

`git cat-file` to examine refs:

    $ git cat-file -p f60b291752bc38be7dfc90c4c4034de13e01a66b

    tree 1afc38724d2b89264c7b3826d40b0655a95cfab4
    author cloudhead <cloudhead@anonymous.xyz> 1678097961 +0100
    committer cloudhead <cloudhead@anonymous.xyz> 1678097961 +0100
    gpgsig -----BEGIN SSH SIGNATURE-----
     U1NIU0lHAAAAAQAAADMAAAALc3NoLWVkMjU1MTkAAAAgvjrQogRxxLjzzWns8+mKJAGzEX
     4fm2ALoN7pyvD2ssQAAAADZ2l0AAAAAAAAAAZzaGE1MTIAAABTAAAAC3NzaC1lZDI1NTE5
     AAAAQHXhUf7QjXNlgCjDbGSG+zoyIlE4S9/d9qjvG7x9jw8J/fXDVIMkh/Lkp743g7EliM
     X+88wqit9BeQoHXuxj2Ao=
     -----END SSH SIGNATURE-----

    Init

You can also run `git ls-remote rad` from inside a working copy to examine the
remote refs in storage.

### Connecting to your local node

The radicle node listens on a UNIX domain socket located at
`$RAD_HOME/node/control.sock`. Make sure this file is accessible and has the
required permissions for your user to read and write to it.

### Radicle keys

Radicle uses Ed25519 keys that are located in `$RAD_HOME/keys`. These keys are
encoded in the standard OpenSSH format. It's therefore possible to use standard
OpenSSH tools to interact with them, eg. `ssh-add`.

Your radicle secret key is protected with a passphrase (See: `$RAD_PASSPHRASE`).

