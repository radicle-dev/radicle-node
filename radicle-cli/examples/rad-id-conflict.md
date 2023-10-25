First let's add Bob as a delegate, and sync the changes to Bob:

``` ~alice
$ rad id update --title "Add Bob" --description "Add Bob as a delegate" --delegate did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk --threshold 2 -q
0ca42d376bd566631083c8913cf86bec722da392
```
``` ~bob
$ cd heartwood
$ rad sync --fetch rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji
✓ Fetching rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji from z6MknSL…StBU8Vi..
✓ Fetched repository from 1 seed(s)
```

One thing that can happen is that two delegates propose a revision at the same
time:

``` ~alice
$ rad id update --title "Edit project name" --description "" --payload "xyz.radicle.project" "name" '"heart"' -q
12d7300d1bbba84e4e5760c8c61999bf5fefb81a
```
``` ~bob
$ rad id update --title "Edit project name" --description "" --payload "xyz.radicle.project" "name" '"wood"' -q
89b2623e7f2ddf5748661b15b9975ab0b4ee17ab
```

When Alice syncs with Bob, she notices the problem: there are two active
revisions.

``` ~alice
$ rad sync --fetch rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji
✓ Fetching rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji from z6Mkt67…v4N1tRk..
✓ Fetched repository from 1 seed(s)
$ rad id list
╭─────────────────────────────────────────────────────────────────────────────────╮
│ ●   ID        Title               Author                     Status     Created │
├─────────────────────────────────────────────────────────────────────────────────┤
│ ●   89b2623   Edit project name   bob      z6Mkt67…v4N1tRk   active     now     │
│ ●   12d7300   Edit project name   alice    (you)             active     now     │
│ ●   0ca42d3   Add Bob             alice    (you)             accepted   now     │
│ ●   0656c21   Initial revision    alice    (you)             accepted   now     │
╰─────────────────────────────────────────────────────────────────────────────────╯
```

This isn't a problem as long as we don't try to accept both. So let's accept
Bob's:

``` ~alice
$ rad id accept 89b2623 -q
$ rad id list
╭─────────────────────────────────────────────────────────────────────────────────╮
│ ●   ID        Title               Author                     Status     Created │
├─────────────────────────────────────────────────────────────────────────────────┤
│ ●   89b2623   Edit project name   bob      z6Mkt67…v4N1tRk   accepted   now     │
│ ●   12d7300   Edit project name   alice    (you)             stale      now     │
│ ●   0ca42d3   Add Bob             alice    (you)             accepted   now     │
│ ●   0656c21   Initial revision    alice    (you)             accepted   now     │
╰─────────────────────────────────────────────────────────────────────────────────╯
```

Doing so voided the other conflicting revision, and it can no longer be
accepted now.

``` ~bob
$ rad sync --fetch rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji
✓ Fetching rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji from z6MknSL…StBU8Vi..
✓ Fetched repository from 1 seed(s)
```
``` ~bob (fail)
$ rad id accept 12d7300 -q
✗ Error: cannot vote on revision that is stale
$ rad id reject 12d7300 -q
✗ Error: cannot vote on revision that is stale
```
``` ~bob
$ rad id show 12d7300
╭────────────────────────────────────────────────────────────────────────╮
│ Title    Edit project name                                             │
│ Revision 12d7300d1bbba84e4e5760c8c61999bf5fefb81a                      │
│ Blob     e93aa3e3c5c448bacd3537a81daf1437eccd046a                      │
│ Author   did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi      │
│ State    stale                                                         │
│ Quorum   no                                                            │
├────────────────────────────────────────────────────────────────────────┤
│ ✓ did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi alice       │
│ ? did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk bob   (you) │
╰────────────────────────────────────────────────────────────────────────╯

@@ -1,14 +1,14 @@
 {
   "payload": {
     "xyz.radicle.project": {
       "defaultBranch": "master",
       "description": "Radicle Heartwood Protocol & Stack",
-      "name": "heartwood"
+      "name": "heart"
     }
   },
   "delegates": [
     "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi",
     "did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk"
   ],
   "threshold": 2
 }
```
