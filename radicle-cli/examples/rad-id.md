At some point in the lifetime of a Radicle project you may want to
collaborate with someone else allowing them to become a project
maintainer. This requires adding them as a `delegate` and possibly
editing the `threshold` for passing new changes to the identity of the
project.

For cases where `threshold > 1`, it is necessary to gather a quorum of
signatures to update the Radicle identity. To do this, we use the `rad id`
command. For now, since we are the only delegate, and `treshold` is `1`, we
can update the identity ourselves.

Let's add Bob as a delegate using their DID,
`did:key:z6MkedTZGJGqgQ2py2b8kGecfxdt2yRdHWF6JpaZC47fovFn`, and update the
threshold to `2`.

```
$ rad id update --title "Add Bob" --description "Add Bob as a delegate" --delegate did:key:z6MkedTZGJGqgQ2py2b8kGecfxdt2yRdHWF6JpaZC47fovFn --threshold 2
✓ Identity revision 07829cdd1993295cd6be18de6219fead428b4a5e created
╭───────────────────────────────────────────────────────────────────╮
│ Title    Add Bob                                                  │
│ Revision 07829cdd1993295cd6be18de6219fead428b4a5e                 │
│ Blob     7109c1c201c223dd4e9fdb10f7330dc6f0310258                 │
│ Author   did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi │
│ State    accepted                                                 │
│ Quorum   yes                                                      │
│                                                                   │
│ Add Bob as a delegate                                             │
├───────────────────────────────────────────────────────────────────┤
│ ✓ did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi  (you) │
╰───────────────────────────────────────────────────────────────────╯

@@ -1,13 +1,14 @@
 {
   "payload": {
     "xyz.radicle.project": {
       "defaultBranch": "master",
       "description": "Radicle Heartwood Protocol & Stack",
       "name": "heartwood"
     }
   },
   "delegates": [
-    "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi"
+    "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi",
+    "did:key:z6MkedTZGJGqgQ2py2b8kGecfxdt2yRdHWF6JpaZC47fovFn"
   ],
-  "threshold": 1
+  "threshold": 2
 }
```

Before moving on, let's take a few notes on this output. The first
thing we'll notice is that the difference between the current identity
document and the proposed changes are shown. Specifically, we changed
the delegates and threshold:

      "delegates": [
    -   "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi"
    +   "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi",
    +   "did:key:z6MkedTZGJGqgQ2py2b8kGecfxdt2yRdHWF6JpaZC47fovFn"
      ],
    ...
    -  "threshold": 1
    +  "threshold": 2

Next we have the number of signatures from delegates, which includes our own:

    ✓ did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi

Finally, we can see whether the `Quorum` was reached:

    Quorum   yes

Since the threshold was previously `1`, this change is now in effect. We
can verify that by listing the current identity document:

```
$ rad inspect --identity
{
  "payload": {
    "xyz.radicle.project": {
      "defaultBranch": "master",
      "description": "Radicle Heartwood Protocol & Stack",
      "name": "heartwood"
    }
  },
  "delegates": [
    "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi",
    "did:key:z6MkedTZGJGqgQ2py2b8kGecfxdt2yRdHWF6JpaZC47fovFn"
  ],
  "threshold": 2
}
```

We can also look at the document's COB directly:
```
$ rad cob show --object 0656c217f917c3e06234771e9ecae53aba5e173e --type xyz.radicle.id --repo rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji
commit 07829cdd1993295cd6be18de6219fead428b4a5e
parent 0656c217f917c3e06234771e9ecae53aba5e173e
author z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
date   Thu, 15 Dec 2022 17:28:04 +0000

    {
      "blob": "7109c1c201c223dd4e9fdb10f7330dc6f0310258",
      "description": "Add Bob as a delegate",
      "parent": "0656c217f917c3e06234771e9ecae53aba5e173e",
      "signature": "z3sne3sdReZ4AtgxQmn7R1pQnz7E9ZEUoRfCJDJ8ytgnBMFW4DJqRHuBz2h1NK4QdGEy3QCpyVoJKfE95tNoivXwz",
      "title": "Add Bob",
      "type": "revision"
    }

commit 0656c217f917c3e06234771e9ecae53aba5e173e
author z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
date   Thu, 15 Dec 2022 17:28:04 +0000

    {
      "blob": "d96f425412c9f8ad5d9a9a05c9831d0728e2338d",
      "parent": null,
      "signature": "z5nGqUvrmfiSyLjNCHWTWYvVMcPUZcvo9TxPKzEKXYBdSgUzbrqf1cYsmpGgbQvYunnsrLSsubEmxZaRdKM4quqQR",
      "title": "Initial revision",
      "type": "revision"
    }

```

Note that once a revision is accepted, it can't be edited, redacted or otherwise
acted upon:

``` (fail)
$ rad id redact 07829cdd1993295cd6be18de6219fead428b4a5e
✗ Error: [..]
```
``` (fail)
$ rad id reject 07829cdd1993295cd6be18de6219fead428b4a5e
✗ Error: [..]
```
``` (fail)
$ rad id accept 07829cdd1993295cd6be18de6219fead428b4a5e
✗ Error: [..]
```
