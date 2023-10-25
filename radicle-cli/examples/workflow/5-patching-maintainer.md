Back to being the project maintainer.

Changes have been proposed by another person (or peer) via a radicle patch.  To
follow changes by another, we must 'track' them.

```
$ rad track did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk --alias bob
✓ Tracking policy updated for z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk (bob)
```

Additionally, we need to add a new 'git remote' to our working copy for the
peer.  Upcoming versions of radicle will not require this step.

```
$ rad remote add z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk --name bob
✓ Remote bob added
✓ Remote-tracking branch bob/master created for z6Mkt67…v4N1tRk
```

``` (stderr)
$ git fetch bob
✓ Synced with 1 peer(s)
From rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk
 * [new branch]      master     -> bob/master
 * [new branch]      patches/3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c -> bob/patches/3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c
```

The contributor's changes are now visible to us.

```
$ git branch -r
  bob/master
  bob/patches/3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c
  rad/master
$ rad patch show 3581e83
╭─────────────────────────────────────────────────────────────────────╮
│ Title    Define power requirements                                  │
│ Patch    3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c                   │
│ Author   bob z6Mkt67…v4N1tRk                                        │
│ Head     27857ec9eb04c69cacab516e8bf4b5fd36090f66                   │
│ Commits  ahead 2, behind 0                                          │
│ Status   open                                                       │
│                                                                     │
│ See details.                                                        │
├─────────────────────────────────────────────────────────────────────┤
│ 27857ec Add README, just for the fun                                │
│ 3e674d1 Define power requirements                                   │
├─────────────────────────────────────────────────────────────────────┤
│ ● opened by bob z6Mkt67…v4N1tRk now                                 │
│ ↑ updated to 6de8527cdf51f96e12649c7278efe1dccfdee885 (27857ec) now │
╰─────────────────────────────────────────────────────────────────────╯
```

Wait! There's a mistake.  The REQUIREMENTS should be a markdown file.  Let's
quickly update the patch before incorporating the changes.  Updating it this
way will tell others about the corrections we needed before merging the
changes.

```
$ rad patch checkout 3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c
✓ Switched to branch patch/3581e83
✓ Branch patch/3581e83 setup to track rad/patches/3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c
$ git mv REQUIREMENTS REQUIREMENTS.md
$ git commit -m "Use markdown for requirements"
[patch/3581e83 f567f69] Use markdown for requirements
 1 file changed, 0 insertions(+), 0 deletions(-)
 rename REQUIREMENTS => REQUIREMENTS.md (100%)
```
``` (stderr)
$ git push rad -o no-sync -o patch.message="Use markdown for requirements"
✓ Patch 3581e83 updated to abb0360eae315bbd460743381303567587ab0e08
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
 * [new branch]      patch/3581e83 -> patches/3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c
```

Great, all fixed up, lets merge the code.

```
$ git checkout master
Your branch is up to date with 'rad/master'.
$ git merge patch/3581e83
Updating f2de534..f567f69
Fast-forward
 README.md       | 0
 REQUIREMENTS.md | 0
 2 files changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 README.md
 create mode 100644 REQUIREMENTS.md
```
``` (stderr)
$ git push rad master
✓ Patch 3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c merged at revision abb0360
✓ Synced with 1 node(s)
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
   f2de534..f567f69  master -> master
```

The patch is now merged and closed :).

```
$ rad patch show 3581e83
╭─────────────────────────────────────────────────────────────────────╮
│ Title    Define power requirements                                  │
│ Patch    3581e83ad18f5cdd806ab50fa11cfd5dd4e8ae1c                   │
│ Author   bob z6Mkt67…v4N1tRk                                        │
│ Head     27857ec9eb04c69cacab516e8bf4b5fd36090f66                   │
│ Commits  ahead 0, behind 1                                          │
│ Status   merged                                                     │
│                                                                     │
│ See details.                                                        │
├─────────────────────────────────────────────────────────────────────┤
│ 27857ec Add README, just for the fun                                │
│ 3e674d1 Define power requirements                                   │
├─────────────────────────────────────────────────────────────────────┤
│ ● opened by bob z6Mkt67…v4N1tRk now                                 │
│ ↑ updated to 6de8527cdf51f96e12649c7278efe1dccfdee885 (27857ec) now │
│ * revised by alice (you) in abb0360 (f567f69) now                   │
│ ✓ merged by alice (you) at revision abb0360 (f567f69) now           │
╰─────────────────────────────────────────────────────────────────────╯
```

To publish our new state to the network, we simply push:

```
$ git push
```
