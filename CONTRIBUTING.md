# CONTRIBUTING

Contributions are very welcome. When contributing code, please follow these
simple guidelines.

* Follow the coding guidelines when proposing code changes ([see below](#code-style)).
* Write properly formatted git commits messages ([see below](#writing-git-commit-messages)).
* Read the DCO and if you are contributing a significant amount of code, make sure your commits are signed off, using `git commit -s`.
* Contribute code via a Radicle Patch, by following the [guide][].
* If it's a feature addition or major change, check with the maintainers first
  before submitting a patch. We wouldn't want you to waste your time!
* If you need help or would like to discuss your changes, come to our community chat on [Zulip][zulip].

[guide]: https://radicle.xyz/guides/user#working-with-patches
[zulip]: https://radicle.zulipchat.com

## Submitting patches

Patch formatting follows the same rules as commit formatting. See below.

## Linting & formatting

Always check your code with the linter (`clippy`), by running:

    $ cargo clippy --workspace --tests

And make sure your code is formatted with, using:

    $ cargo fmt

Finally, ensure there is no trailing whitespace anywhere.

## Running tests

Make sure all tests are passing with:

    $ cargo test --workspace

## Checking the docs

If you make documentation changes, you may want to check whether there are any
warnings or errors:

    $ cargo doc --workspace --all-features

## Code style

The following code guidelines will help make code review smoother.

### Use of `unwrap` and `expect`

Use `unwrap` only in either of three circumstances:

1. Based on manual static analysis, you've concluded that it's impossible for
the code to panic; so unwrapping is *safe*. An example would be:

        let list = vec![a, b, c];
        let first = list.first().unwrap();

2. The panic caused by `unwrap` would indicate a bug in the software, and it
would be impossible to continue in that case.

3. The `unwrap` is part of test code, ie. `cfg!(test)` is `true`.

In the first and second case, document `unwrap` call sites with a comment prefixed
with `SAFETY:` that explains why it's safe to unwrap, eg.

    // SAFETY: Node IDs are valid ref strings.
    let r = RefString::try_from(node.to_string()).unwrap();

Use `expect` only if the function expects certain invariants that were not met,
either due to bad inputs, or a problem with the environment; and include the
expectation in the message. For example:

    logger::init(log::Level::Debug)
        .expect("logger must only be initialized once");

### Module imports

Modules are declared at the top of the file, before the imports. Public modules
are separated from private modules with a blank line:

    mod git;
    mod storage;

    pub mod refs;

    use std::time;
    use std::process;

    ...

Imports are organized in groups, from least specific to more specific:

    use std::collections::HashMap;   // First, `std` imports.
    use std::process;
    use std::time;

    use git_ref_format as format;    // Then, external dependencies.
    use once_cell::sync::Lazy;

    use crate::crypto::PublicKey;    // Finally, local crate imports.
    use crate::storage::refs::Refs;
    use crate::storage::RemoteId;

### Variable naming

Use short 1-letter names when the variable scope is only a few lines, or the context is
obvious, eg.

    if let Some(e) = result.err() {
        ...
    }

Use 1-word names for function parameters or variables that have larger scopes:

    pub fn commit(repo: &Repository, sig: &Signature) -> Result<Commit, Error> {
        ...
    }

Use the most descriptive names for globals:

    pub const KEEP_ALIVE_DELTA: LocalDuration = LocalDuration::from_secs(30);

### Function naming

Stay concise. Use the function doc comment to describe
what the function does, not the name. Keep in mind functions are in the
context of the parent module and/or object and repeating that would be
redundant.

### Logging

When writing log statements, always include a `target` and include enough
context in the log message that it is useful on its own, eg.

    debug!(target: "service", "Routing table updated for {rid} with seed {nid}");

Check the file you are working on for what the `target` name should be; most
logs should be at the *debug* level.

## Dependencies

Before adding any code dependencies, check with the maintainers if this is okay.
In general, we try not to add external dependencies unless it's necessary.
Dependencies increase counter-party risk, build-time, attack surface, and
make code harder to audit.

## Documentation

Public types and functions should be documented. Modules *may* be documented,
if you see the need.

Code comments should usually be full english sentences, and add missing context
for the reader:

    // Ensure that our inventory is recorded in our routing table, and we are tracking
    // all of it. It can happen that inventory is not properly tracked if for eg. the
    // user creates a new repository while the node is stopped.
    for rid in self.storage.inventory()? {
        ...

## Proposing changes

When proposing changes via a patch:

* Isolate changes in separate commits to make the review process easier.
* Don't make unrelated changes, unless it happens to be an obvious improvement to
  code you are touching anyway ("boyscout rule").
* Rebase on `master` when needed.
* Keep your changesets small, specific and uncontroversial, so that they can be
  merged more quickly.
* If the change is substantial or requires re-architecting certain parts of the
  codebase, write a proposal in English first, and get consensus on that before
  proposing the code changes.

**Preparing commits**

1. Each commit in your patch must pass all the tests, lints and checks. This is
   so that they can be built into binaries and to make git bisecting possible.
2. Do not include any commits that are fixes or refactorings of previous patch
   commits. These should be squashed to the minimal diff required to make the
   change, unless it's helpful to make a large change over multiple commits,
   while still respecting (1). Do not include `fixup!` commits either.
3. A commit *may* include a category prefix such as `cli:` or `node:` if it
   mainly concerns a certain area of the codebase. For example. These prefixes
   should usually be the name of the crate, minus any common prefix. Eg.
   `cli:`, and *not* `radicle-cli:`. For documentation, you can use `docs:`,
   and for CI-related files, you can use `ci:`.

To help with the above, use `git commit --amend` and `git rebase -i`. You can
also interactively construct a commit from a working tree using `git add -p`.

## Writing commit messages

A properly formed git commit subject line should always be able to complete the
following sentence:

     If applied, this commit will _____

In addition, it should be capitalized and *must not* include a period.

For example, the following message is well formed:

     Add support for .gif files

While these ones are **not**: `Adding support for .gif files`,
`Added support for .gif files`, `add support for .gif files`.

When it comes to formatting, here's a model git commit message[1]:

     Capitalized, short (50 chars or less) summary

     More detailed explanatory text, if necessary.  Wrap it to about 72
     characters or so.  In some contexts, the first line is treated as the
     subject of an email and the rest of the text as the body.  The blank
     line separating the summary from the body is critical (unless you omit
     the body entirely); tools like rebase can get confused if you run the
     two together.

     Write your commit message in the imperative: "Fix bug" and not "Fixed bug"
     or "Fixes bug."  This convention matches up with commit messages generated
     by commands like git merge and git revert.

     Further paragraphs come after blank lines.

     - Bullet points are okay, too.

     - Typically a hyphen or asterisk is used for the bullet, followed by a
       single space, with blank lines in between, but conventions vary here.

     - Use a hanging indent.

---

[1]: http://tbaggery.com/2008/04/19/a-note-about-git-commit-messages.html
