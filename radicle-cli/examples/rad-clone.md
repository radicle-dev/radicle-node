To create a local copy of a repository on the radicle network, we use the
`clone` command, followed by the identifier or *RID* of the repository:

```
$ rad clone rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji --scope trusted
✓ Tracking relationship established for rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji with scope 'trusted'
✓ Fetching rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji from z6MknSL…StBU8Vi..
✓ Forking under z6Mkt67…v4N1tRk..
✓ Creating checkout in ./heartwood..
✓ Remote alice@z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi added
✓ Remote-tracking branch alice@z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi/master created for z6MknSL…StBU8Vi
✓ Repository successfully cloned under [..]/heartwood/
╭────────────────────────────────────╮
│ heartwood                          │
│ Radicle Heartwood Protocol & Stack │
│ 0 issues · 0 patches               │
╰────────────────────────────────────╯
Run `cd ./heartwood` to go to the project directory.
```

We can now have a look at the new working copy that was created from the cloned
repository:

```
$ cd heartwood
$ ls -I
README
$ cat README
Hello World!
```

Let's check that the remote tracking branch was setup correctly:

```
$ git branch --remotes
  alice@z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi/master
  rad/master
```

The first branch is ours, and the second points to the repository delegate.
We can also take a look at the remotes:

```
$ git remote -v
alice@z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi	rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi (fetch)
alice@z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi	rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi (push)
rad	rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji (fetch)
rad	rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk (push)
```

Let's check the last commit!

```
$ git log -n 1
commit f2de534b5e81d7c6e2dcaf58c3dd91573c0a0354
Author: anonymous <anonymous@radicle.xyz>
Date:   Mon Jan 1 14:39:16 2018 +0000

    Second commit
```
