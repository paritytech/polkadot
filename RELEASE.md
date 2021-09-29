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
  to v0.8.26-rc2) and new wasms built. If you need to merge several changes into
a new release-candidate, it's advised to do this as a single PR against the rc branch
* There is a step on the auto-created github issue checklist that specifies we should
  push the new runtime to Westend. This can be done either before or after publishing
  the release, but if there are new host functions in the client, it is advisable to 
  do this *after* releasing, so node operators have a chance to upgrade their nodes
  prior to the new runtime going live (new host functions can cause chains to stop
  syncing on nodes that have not been updated)

### Release workflow

Below are the steps of the release workflow. Steps prefixed with NOACTION are
automated and require no human action.

1. To initiate the release process, branch master off to a release branch and push it to Github:
  - `git checkout master; git pull; git checkout -b release-v0.8.26; git push origin refs/heads/release-v0.8.26`
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
7. Once happy with the release-candidate (i.e., the checks in the auto-created
  checklist are completed) tag the tip of the release-candidate branch:
  - `git tag -s -m 'v0.8.26' v0.8.26; git push --tags`

### Security releases

Occasionally there may be changes that need to be made to the most recently
released version of Polkadot, without taking *every* change to `master` since
the last release. For example, in the event of a security vulnerability being
found, where releasing a fixed version is a matter of some expediency. In cases
like this, the fix should first be merged with master, cherry-picked to a branch
forked from the last release-candidate branch, tested, and then finally merged
with the release-candidate branch. A sensible versioning scheme for changes like
this is `vX.Y.Z-1`, though this can cause problems with the RPM packaging.
