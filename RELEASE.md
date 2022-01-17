Polkadot Release Process
------------------------

### Branches
* release-candidate branch: The branch used for staging of the next release.
  Named like `release-v0.8.26`
  
### Notes
* The release-candidate branch *must* be made in the paritytech/polkadot repo in
order for release automation to work correctly
* Any new pushes/merges to the release-candidate branch (for example,
refs/heads/release-v0.8.26) will result in the rc index being bumped (e.g., v0.8.26-rc1
to v0.8.26-rc2) and new wasms built.

### Release workflow

Below are the steps of the release workflow. Steps prefixed with NOACTION are
automated and require no human action.

1. To initiate the release process:
  1. branch master off to a release candidate branch:
  - `git checkout master; git pull; git checkout -b release-v0.8.26`
  2. In the [substrate](https://github.com/paritytech/substrate) repo, check out the commit used by polkadot (this can be found using the following command in the *polkadot* repo: `grep 'paritytech/substrate' Cargo.lock | grep -E '[0-9a-f]{40}' | sort | uniq `
  3. Branch off this **substrate** commit into its own branch: `git branch -b polkadot-v0.8.26; git push origin refs/heads/polkadot-v0.8.26`
  4. In the **polkadot** repository, use [diener](https://github.com/bkchr/diener/) to switch to this branch: `diener update --branch "polkadot-v0.8.26" --substrate`. Update Cargo.lock (to do this, you can run `cargo build` and then ctrl+c once it finishes fetching and begins compiling)
  5. Push the **polkadot** `release-v0.8.26` branch to Github: `git push origin refs/heads/release-v0.8.26`
2. NOACTION: The current HEAD of the release-candidate branch is tagged `v0.8.26-rc1`
3. NOACTION: A draft release and runtime WASMs are created for this
  release-candidate automatically. A link to the draft release will be linked in
  the internal polkadot matrix channel.
4. NOACTION: A new Github issue is created containing a checklist of manual
  steps to be completed before we are confident with the release. This will be
  linked in Matrix.
5. Complete the steps in the issue created in step 4, signing them off as
  completed
6. (optional) If a fix is required to the release-candidate:
  1. Merge the fix with `master` first
  2. Cherry-pick the commit from `master` to `release-v0.8.26`, fixing any
  merge conflicts. Try to avoid unnecessarily bumping crates.
  3. Push the release-candidate branch to Github - this is now the new release-
  candidate
  4. Depending on the cherry-picked changes, it may be necessary to perform some
  or all of the manual tests again.
  5. If there are **substrate** changes required, these should be cherry-picked to the substrate `polkadot-v0.8.26` branch and pushed, and the version of substrate used in **polkadot** updated using `cargo update -p sp-io`
7. Once happy with the release-candidate, tag the current top commit in the release candidate branch and push to Github: `git tag -s -m 'v0.8.26' v0.8.26; git push --tags`
9. NOACTION: The HEAD of the `release` branch will be tagged with `v0.8.26`,
  and a final draft release will be created on Github.

### Security releases

Occasionally there may be changes that need to be made to the most recently
released version of Polkadot, without taking *every* change to `master` since
the last release. For example, in the event of a security vulnerability being
found, where releasing a fixed version is a matter of some expediency. In cases
like this, the fix should first be merged with master, cherry-picked to a branch
forked from `release`, tested, and then finally merged with `release`. A
sensible versioning scheme for changes like this is `vX.Y.Z-1`.
