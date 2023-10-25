We may want to work on top of an existing patch and this where `rad
patch checkout` comes into play. So, first we will create a patch to
set up the workflow.

```
$ git checkout -b flux-capacitor-power
$ touch REQUIREMENTS
```

Here the instructions are added to the project's README for 1.21 gigawatts and
commit the changes to git.

```
$ git add REQUIREMENTS
$ git commit -v -m "Define power requirements"
[flux-capacitor-power 3e674d1] Define power requirements
 1 file changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 REQUIREMENTS
```

Once the code is ready, we open (or create) a patch with our changes for the project.

``` (stderr)
$ git push rad -o patch.message="Define power requirements" -o patch.message="See details." HEAD:refs/patches
✓ Patch 6ff4f09c1b5a81347981f59b02ef43a31a07cdae opened
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
 * [new reference]   HEAD -> refs/patches
```

Now, let's checkout the patch that we just created:

```
$ rad patch checkout 6ff4f09
✓ Switched to branch patch/6ff4f09
✓ Branch patch/6ff4f09 setup to track rad/patches/6ff4f09c1b5a81347981f59b02ef43a31a07cdae
```

Now, let's add a README too!

```
$ touch README.md
$ git add README.md
$ git commit --message "Add README, just for the fun"
[patch/6ff4f09 27857ec] Add README, just for the fun
 1 file changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 README.md
```

But maybe we first wanted to rebase `master` so we ended up being on
that branch:

``` (stderr)
$ git checkout master
Switched to branch 'master'
```

We can be safe in the knowledge that our changes on the
`patch/6ff4f09` branch are still safe:

```
$ rad patch checkout 6ff4f09
✓ Switched to branch patch/6ff4f09
```

We can now finish off the update:

``` (stderr)
$ git push rad -o patch.message="Add README, just for the fun"
✓ Patch 6ff4f09 updated to 0c0942e2ff2488617d950ede15567ca39a29972e
To rad://z42hL2jL4XNk6K8oHQaSWfMgCL7ji/z6MknSLrJoTcukLrE435hVNQT4JUhbvWLX4kUzqkEStBU8Vi
   3e674d1..27857ec  patch/6ff4f09 -> patches/6ff4f09c1b5a81347981f59b02ef43a31a07cdae
```
