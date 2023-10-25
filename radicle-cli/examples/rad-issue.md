Project 'todo' items are called 'issue's.  They can be inspected and modified
using the 'issue' subcommand.

Let's say the new car you are designing with your peers has a problem with its flux capacitor.

```
$ rad issue open --title "flux capacitor underpowered" --description "Flux capacitor power requirements exceed current supply" --no-announce
╭─────────────────────────────────────────────────────────╮
│ Title   flux capacitor underpowered                     │
│ Issue   d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61        │
│ Author  z6MknSL…StBU8Vi (you)                           │
│ Status  open                                            │
│                                                         │
│ Flux capacitor power requirements exceed current supply │
╰─────────────────────────────────────────────────────────╯
```

The issue is now listed under our project.

```
$ rad issue list
╭───────────────────────────────────────────────────────────────────────────────────────────────────╮
│ ●   ID        Title                         Author                    Labels   Assignees   Opened │
├───────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ●   d185ee1   flux capacitor underpowered   z6MknSL…StBU8Vi   (you)                        now    │
╰───────────────────────────────────────────────────────────────────────────────────────────────────╯
```

Show the issue information issue.

```
$ rad issue show d185ee1
╭─────────────────────────────────────────────────────────╮
│ Title   flux capacitor underpowered                     │
│ Issue   d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61        │
│ Author  z6MknSL…StBU8Vi (you)                           │
│ Status  open                                            │
│                                                         │
│ Flux capacitor power requirements exceed current supply │
╰─────────────────────────────────────────────────────────╯
```


Great! Now we've documented the issue for ourselves and others.

Just like with other project management systems, the issue can be assigned to
others to work on.  This is to ensure work is not duplicated.

Let's assign ourselves to this one.

```
$ rad assign d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 --to did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
```

It will now show in the list of issues assigned to us.

```
$ rad issue list --assigned
╭─────────────────────────────────────────────────────────────────────────────────────────────────────────╮
│ ●   ID        Title                         Author                    Labels   Assignees         Opened │
├─────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ●   d185ee1   flux capacitor underpowered   z6MknSL…StBU8Vi   (you)            z6MknSL…StBU8Vi   now    │
╰─────────────────────────────────────────────────────────────────────────────────────────────────────────╯
```

Note: this can always be undone with the `unassign` subcommand.

```
$ rad unassign d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 --from did:key:z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
```

Great, now we have communicated to the world about our car's defect.

But wait! We've found an important detail about the car's power requirements.
It will help whoever works on a fix.

```
$ rad issue comment d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 --message 'The flux capacitor needs 1.21 Gigawatts' -q
14019611935fd1c66458111a5b49f0a7350c226f
$ rad issue comment d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61 --reply-to 14019611935fd1c66458111a5b49f0a7350c226f --message 'More power!' -q
e342e47b7dd93451d47c806a0faeb0b5e957da2c
```

We can see our comments by showing the issue:

```
$ rad issue show d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61
╭─────────────────────────────────────────────────────────╮
│ Title   flux capacitor underpowered                     │
│ Issue   d185ee16a00bac874c0bcbc2a8ad80fdce5e1e61        │
│ Author  z6MknSL…StBU8Vi (you)                           │
│ Status  open                                            │
│                                                         │
│ Flux capacitor power requirements exceed current supply │
├─────────────────────────────────────────────────────────┤
│ z6MknSL…StBU8Vi (you) now 1401961                       │
│ The flux capacitor needs 1.21 Gigawatts                 │
├─────────────────────────────────────────────────────────┤
│ z6MknSL…StBU8Vi (you) now e342e47                       │
│ More power!                                             │
╰─────────────────────────────────────────────────────────╯
```
