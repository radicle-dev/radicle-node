Let's explore the `rad patch update` plumbing command. First we create a patch:

``` (stderr)
$ git checkout -q -b feature/1
$ git commit -q -m "Not a real change" --allow-empty
```
``` (stderr)
$ git push rad HEAD:refs/patches
✓ Patch 8f5dcedc07a89928fd450bce1479f2559bcfd1d4 opened
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
 * [new reference]   HEAD -> refs/patches
```

```
$ rad patch show 8f5dcedc07a89928fd450bce1479f2559bcfd1d4
╭────────────────────────────────────────────────────╮
│ Title     Not a real change                        │
│ Patch     8f5dcedc07a89928fd450bce1479f2559bcfd1d4 │
│ Author    z6MknSL…StBU8Vi (you)                    │
│ Head      51b2f0f77b9849bfaa3e9d3ff68ee2f57771d20c │
│ Branches  feature/1                                │
│ Commits   ahead 1, behind 0                        │
│ Status    open                                     │
├────────────────────────────────────────────────────┤
│ 51b2f0f Not a real change                          │
├────────────────────────────────────────────────────┤
│ ● opened by z6MknSL…StBU8Vi (you) now              │
╰────────────────────────────────────────────────────╯
```

We can make some changes to the repository:

```
$ git mv README README.md
$ git commit -q -m "Rename readme file"
```

Let's push the changes, but not to the magic ref, that way the push doesn't
update our patch:

```
$ git push rad HEAD:refs/heads/feature/1
```

Now, instead of using `git push` to update the patch, as we normally would,
we run:

```
$ rad patch update 8f5dcedc07a89928fd450bce1479f2559bcfd1d4 -m "Updated patch"
74d453f93d81bb535ffa4ef65c46e5bd0a76015d
```

The command outputs the new Revision ID, which we can now see here:

```
$ rad patch show 8f5dcedc07a89928fd450bce1479f2559bcfd1d4
╭─────────────────────────────────────────────────────────────────────╮
│ Title     Not a real change                                         │
│ Patch     8f5dcedc07a89928fd450bce1479f2559bcfd1d4                  │
│ Author    z6MknSL…StBU8Vi (you)                                     │
│ Head      4d272148458a17620541555b1f0905c01658aa9f                  │
│ Branches  feature/1                                                 │
│ Commits   ahead 2, behind 0                                         │
│ Status    open                                                      │
├─────────────────────────────────────────────────────────────────────┤
│ 4d27214 Rename readme file                                          │
│ 51b2f0f Not a real change                                           │
├─────────────────────────────────────────────────────────────────────┤
│ ● opened by z6MknSL…StBU8Vi (you) now                               │
│ ↑ updated to 74d453f93d81bb535ffa4ef65c46e5bd0a76015d (4d27214) now │
╰─────────────────────────────────────────────────────────────────────╯
```
