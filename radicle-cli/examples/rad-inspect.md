To display a repository's identifier, or *RID*, you may use the `rad inspect`
command from inside a working copy:

```
$ rad inspect
rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji
```

As a shorthand, you can also simply use `rad .`:

```
$ rad .
rad:z42hL2jL4XNk6K8oHQaSWfMgCL7ji
```

It's also possible to display all of the repository's git references:

```
$ rad inspect --refs
z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
└── refs
    ├── cobs
    │   └── xyz.radicle.id
    │       └── 0656c217f917c3e06234771e9ecae53aba5e173e
    ├── heads
    │   └── master
    └── rad
        ├── id
        └── sigrefs
```

Or display the repository identity's payload and delegates:

```
$ rad inspect --payload
{
  "xyz.radicle.project": {
    "defaultBranch": "master",
    "description": "Radicle Heartwood Protocol & Stack",
    "name": "heartwood"
  }
}
$ rad inspect --delegates
did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
```

Finally, the `--history` flag allows you to examine the identity document's
history:

```
$ rad inspect --history
commit 0656c217f917c3e06234771e9ecae53aba5e173e
blob   d96f425412c9f8ad5d9a9a05c9831d0728e2338d
date   Thu, 15 Dec 2022 17:28:04 +0000

    Initialize identity

 {
   "payload": {
     "xyz.radicle.project": {
       "defaultBranch": "master",
       "description": "Radicle Heartwood Protocol & Stack",
       "name": "heartwood"
     }
   },
   "delegates": [
     "did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi"
   ],
   "threshold": 1
 }

```

The identity document is the metadata associated with a repository, that is
only changeable by delegates.
