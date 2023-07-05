When contributing to another's project, it is common for the contribution to be
of many commits and involve a discussion with the project's maintainer.  This is supported
via Radicle *patches*.

Here we give a brief overview for using patches in our hypothetical car
scenario.  It turns out instructions containing the power requirements were
missing from the project.

```
$ git checkout -b flux-capacitor-power
$ touch REQUIREMENTS
```

Here the instructions are added to the project's `REQUIREMENTS` for 1.21
gigawatts and committed with git.

```
$ git add REQUIREMENTS
$ git commit -v -m "Define power requirements"
[flux-capacitor-power 3e674d1] Define power requirements
 1 file changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 REQUIREMENTS
```

Once the code is ready, we open a patch with our changes.

``` (stderr)
$ git push rad -o no-sync -o patch.message="Define power requirements" -o patch.message="See details." HEAD:refs/patches
✓ Patch 189af83ecb7f0405209ae8275af45816a4c630b7 opened
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk
 * [new reference]   HEAD -> refs/patches
```

It will now be listed as one of the project's open patches.

```
$ rad patch
╭──────────────────────────────────────────────────────────────────────────────────────────────────╮
│ ●  ID       Title                      Author                      Head     +   -   Updated      │
├──────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ●  189af83  Define power requirements  z6Mkt67…v4N1tRk  bob (you)  3e674d1  +0  -0  [    ...   ] │
╰──────────────────────────────────────────────────────────────────────────────────────────────────╯
$ rad patch show 189af83ecb7f0405209ae8275af45816a4c630b7
╭────────────────────────────────────────────────────────────────────╮
│ Title     Define power requirements                                │
│ Patch     189af83ecb7f0405209ae8275af45816a4c630b7                 │
│ Author    did:key:z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk │
│ Head      3e674d1a1df90807e934f9ae5da2591dd6848a33                 │
│ Branches  flux-capacitor-power                                     │
│ Commits   ahead 1, behind 0                                        │
│ Status    open                                                     │
│                                                                    │
│ See details.                                                       │
├────────────────────────────────────────────────────────────────────┤
│ 3e674d1 Define power requirements                                  │
├────────────────────────────────────────────────────────────────────┤
│ ● opened by bob (you) [   ...    ]                                 │
╰────────────────────────────────────────────────────────────────────╯
```

We can also confirm that the patch branch is in storage:

```
$ git ls-remote rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk refs/heads/patches/*
3e674d1a1df90807e934f9ae5da2591dd6848a33	refs/heads/patches/189af83ecb7f0405209ae8275af45816a4c630b7
```

Wait, let's add a README too! Just for fun.

```
$ touch README.md
$ git add README.md
$ git commit --message "Add README, just for the fun"
[flux-capacitor-power 27857ec] Add README, just for the fun
 1 file changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 README.md
```
``` (stderr) RAD_SOCKET=/dev/null
$ git push -o patch.message="Add README, just for the fun"
✓ Patch 189af83 updated to 74480f123adb5b3783a9da4e647658b9ffe87630
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6Mkt67GdsW7715MEfRuP4pSZxJRJh6kj6Y48WRqVv4N1tRk
   3e674d1..27857ec  flux-capacitor-power -> patches/189af83ecb7f0405209ae8275af45816a4c630b7
```

And let's leave a quick comment for our team:

```
$ rad comment 189af83ecb7f0405209ae8275af45816a4c630b7 --message 'I cannot wait to get back to the 90s!'
72cfda6eb7bfbb7ee12b5ab8b79b3253111a6828
```
