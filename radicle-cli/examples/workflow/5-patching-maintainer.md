Back to being the project maintainer.

Changes have been proposed by another person (or peer) via a radicle patch.  To follow changes by another, we must 'track' them.

```
$ rad track did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk --alias bob
✓ Tracking policy updated for z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk (bob)
$ rad sync --fetch
✓ Fetching rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji from z6Mkt67…v4N1tRk..
✓ Fetched repository from 1 seed(s)
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
From rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk
 * [new branch]      master     -> bob/master
 * [new branch]      patches/189af83ecb7f0405209ae8275af45816a4c630b7 -> bob/patches/189af83ecb7f0405209ae8275af45816a4c630b7
```

The contributor's changes are now visible to us.

```
$ git branch -r
  bob/master
  bob/patches/189af83ecb7f0405209ae8275af45816a4c630b7
  rad/master
$ rad patch show 189af83
╭──────────────────────────────────────────────────────────────────────────────╮
│ Title    Define power requirements                                           │
│ Patch    189af83ecb7f0405209ae8275af45816a4c630b7                            │
│ Author   did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk            │
│ Head     27857ec9eb04c69cacab516e8bf4b5fd36090f66                            │
│ Commits  ahead 2, behind 0                                                   │
│ Status   open                                                                │
│                                                                              │
│ See details.                                                                 │
├──────────────────────────────────────────────────────────────────────────────┤
│ 27857ec Add README, just for the fun                                         │
│ 3e674d1 Define power requirements                                            │
├──────────────────────────────────────────────────────────────────────────────┤
│ ● opened by bob (z6Mkt67…v4N1tRk) [   ...    ]                               │
│ ↑ updated to 74480f123adb5b3783a9da4e647658b9ffe87630 (27857ec) [   ...    ] │
╰──────────────────────────────────────────────────────────────────────────────╯
```

Wait! There's a mistake.  The REQUIREMENTS should be a markdown file.  Let's
quickly update the patch before incorporating the changes.  Updating it this
way will tell others about the corrections we needed before merging the
changes.

```
$ rad patch checkout 189af83ecb7f0405209ae8275af45816a4c630b7
✓ Switched to branch patch/189af83
✓ Branch patch/189af83 setup to track rad/patches/189af83ecb7f0405209ae8275af45816a4c630b7
$ git mv REQUIREMENTS REQUIREMENTS.md
$ git commit -m "Use markdown for requirements"
[patch/189af83 f567f69] Use markdown for requirements
 1 file changed, 0 insertions(+), 0 deletions(-)
 rename REQUIREMENTS => REQUIREMENTS.md (100%)
```
``` (stderr)
$ git push rad -o no-sync -o patch.message="Use markdown for requirements"
✓ Patch 189af83 updated to ce2c55fc6736f64fb7f9f1c0058b4f1d20bd54a5
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
 * [new branch]      patch/189af83 -> patches/189af83ecb7f0405209ae8275af45816a4c630b7
```

Great, all fixed up, lets merge the code.

```
$ git checkout master
Your branch is up to date with 'rad/master'.
$ git merge patch/189af83
Updating f2de534..f567f69
Fast-forward
 README.md       | 0
 REQUIREMENTS.md | 0
 2 files changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 README.md
 create mode 100644 REQUIREMENTS.md
$ git push rad master
```

The patch is now merged and closed :).

```
$ rad patch show 189af83
╭──────────────────────────────────────────────────────────────────────────────╮
│ Title     Define power requirements                                          │
│ Patch     189af83ecb7f0405209ae8275af45816a4c630b7                           │
│ Author    did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk           │
│ Head      f567f695d25b4e8fb63b5f5ad2a584529826e908                           │
│ Branches  master, patch/189af83                                              │
│ Commits   up to date                                                         │
│ Status    merged                                                             │
│                                                                              │
│ See details.                                                                 │
├──────────────────────────────────────────────────────────────────────────────┤
│ f567f69 Use markdown for requirements                                        │
│ 27857ec Add README, just for the fun                                         │
│ 3e674d1 Define power requirements                                            │
├──────────────────────────────────────────────────────────────────────────────┤
│ ● opened by bob (z6Mkt67…v4N1tRk) [   ...    ]                               │
│ ↑ updated to 74480f123adb5b3783a9da4e647658b9ffe87630 (27857ec) [   ...    ] │
│ ↑ updated to ce2c55fc6736f64fb7f9f1c0058b4f1d20bd54a5 (f567f69) [   ...    ] │
│ ✓ merged by alice (you) [   ...    ]                                         │
╰──────────────────────────────────────────────────────────────────────────────╯
```

To publish our new state to the network, we simply push:

```
$ git push
```
