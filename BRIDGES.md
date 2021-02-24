# Using Parity Bridges Common dependency (`git subtree`).

In `./bridges` sub-directory you can find a `git subtree` imported version of:
[parity-bridges-common](https://github.com/paritytech/parity-bridges-common/) repository.

# How to fix broken Bridges code?

To fix Bridges code simply create a commit in current (`polkadot`) repo. Best if
the commit is isolated to changes in `./bridges` sub-directory, because it makes
it easier to import that change back to upstream repo.

# How to pull latest Bridges code or contribute back?

Note that it's totally fine to ping the Bridges Team to do that for you. The point
of adding the code as `git subtree` is to **reduce maintenance cost** for Polkadot
developers.

If you still would like to either update the code to match latest code from the repo
or create an upstream PR read below. The following commands should be run in the 
current (`polkadot`) repo.

1. Add Bridges repo as a local remote:
```
$ git remote add -f bridges git@github.com:paritytech/parity-bridges-common.git
```

If you plan to contribute back, consider forking the repository on Github and adding
your personal fork as a remote as well.
```
$ git remote add -f my-bridges git@github.com:tomusdrw/parity-bridges-common.git
```

2. To update Bridges:
```
$ git fetch bridges master
$ git subtree pull --prefix=bridges bridges master --squash
````

We use `--squash` to avoid adding individual commits and rather squashing them
all into one.

3. Contributing back to Bridges (creating upstream PR)
```
$ git subtree push --prefix=bridges my-bridges master
```
This command will push changes to your personal fork of Bridges repo, from where
you can simply create a PR to the main repo.
