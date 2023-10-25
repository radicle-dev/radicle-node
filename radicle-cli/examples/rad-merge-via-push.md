Let's start by creating two patches.

``` (stderr) RAD_SOCKET=/dev/null
$ git checkout -b feature/1 -q
$ git commit --allow-empty -q -m "First change"
$ git push rad HEAD:refs/patches
✓ Patch a1207f6e82700e42cc46c9c38c7786b18cbd2040 opened
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
 * [new reference]   HEAD -> refs/patches
```
``` (stderr) RAD_SOCKET=/dev/null
$ git checkout -b feature/2 -q master
$ git commit --allow-empty -q -m "Second change"
$ git push rad HEAD:refs/patches
✓ Patch 8357a9f1d61e80309d314491aa754969d9f47d77 opened
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
 * [new reference]   HEAD -> refs/patches
```

This creates some remote tracking branches for us:

```
$ git branch -r
  rad/master
  rad/patches/8357a9f1d61e80309d314491aa754969d9f47d77
  rad/patches/a1207f6e82700e42cc46c9c38c7786b18cbd2040
```

And some remote refs:

```
$ rad inspect --refs
z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
└── refs
    ├── cobs
    │   ├── xyz.radicle.id
    │   │   └── 0656c217f917c3e06234771e9ecae53aba5e173e
    │   └── xyz.radicle.patch
    │       ├── 8357a9f1d61e80309d314491aa754969d9f47d77
    │       └── a1207f6e82700e42cc46c9c38c7786b18cbd2040
    ├── heads
    │   ├── master
    │   └── patches
    │       ├── 8357a9f1d61e80309d314491aa754969d9f47d77
    │       └── a1207f6e82700e42cc46c9c38c7786b18cbd2040
    └── rad
        ├── id
        └── sigrefs
```

Then let's merge the changes into `master`.

``` (stderr) RAD_SOCKET=/dev/null
$ git checkout master
Switched to branch 'master'
$ git merge feature/1
$ git merge feature/2
```

When we push to `rad/master`, we automatically merge the patches:

``` (stderr) RAD_SOCKET=/dev/null
$ git push rad master
✓ Patch 8357a9f1d61e80309d314491aa754969d9f47d77 merged
✓ Patch a1207f6e82700e42cc46c9c38c7786b18cbd2040 merged
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
   f2de534..d6399c7  master -> master
```
```
$ rad patch --merged
╭────────────────────────────────────────────────────────────────────╮
│ ●  ID       Title          Author         Head     +   -   Updated │
├────────────────────────────────────────────────────────────────────┤
│ ✔  [ ... ]  Second change  alice   (you)  daf349f  +0  -0  now     │
│ ✔  [ ... ]  First change   alice   (you)  20aa5dd  +0  -0  now     │
╰────────────────────────────────────────────────────────────────────╯
```

We can verify that the remote tracking branches were also deleted:

```
$ git branch -r
  rad/master
```

And so were the remote branches:

```
$ rad inspect --refs
z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
└── refs
    ├── cobs
    │   ├── xyz.radicle.id
    │   │   └── 0656c217f917c3e06234771e9ecae53aba5e173e
    │   └── xyz.radicle.patch
    │       ├── 8357a9f1d61e80309d314491aa754969d9f47d77
    │       └── a1207f6e82700e42cc46c9c38c7786b18cbd2040
    ├── heads
    │   └── master
    └── rad
        ├── id
        └── sigrefs
```
