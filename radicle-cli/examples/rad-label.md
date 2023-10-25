Labeling an issue is easy, let's add the `bug` and `good-first-issue` labels to
some issue:

```
$ rad label d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 bug good-first-issue
```

We can now show the issue to check whether those labels were added:

```
$ rad issue show d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 --format header
╭─────────────────────────────────────────────────────────╮
│ Title   flux capacitor underpowered                     │
│ Issue   d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61        │
│ Author  z6MknSL…StBU8Vi (you)                           │
│ Labels  bug, good-first-issue                           │
│ Status  open                                            │
│                                                         │
│ Flux capacitor power requirements exceed current supply │
╰─────────────────────────────────────────────────────────╯
```

Untagging an issue is very similar:

```
$ rad unlabel d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 good-first-issue
```

Notice that the `good-first-issue` label has disappeared:

```
$ rad issue show d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 --format header
╭─────────────────────────────────────────────────────────╮
│ Title   flux capacitor underpowered                     │
│ Issue   d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61        │
│ Author  z6MknSL…StBU8Vi (you)                           │
│ Labels  bug                                             │
│ Status  open                                            │
│                                                         │
│ Flux capacitor power requirements exceed current supply │
╰─────────────────────────────────────────────────────────╯
```
