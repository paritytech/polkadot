Polkadot Release Process
------------------------

This is currently a work-in-progress document subject to change at any time.

### Definitions
* release-candidate branch: The branch used for staging of the next release.
  Named like `v0.8.12-rc1` where v0.8.12 is the next version and rc1 is the first
  release candidate for that release.
* release branch: The branch to which successful release-candidates are merged
  and tagged with the new version. Named literally `release`.
* 'generate a [draft] release': Generally means create an entry on this page:
    https://github.com/paritytech/polkadot/releases

### Notes
* Any new pushes/merges to the release-candidate branch (for example, 
refs/heads/v0.8.12) will result in the rc index being bumped (e.g., v0.8.12-rc1
to v0.8.12-rc2), with new tags and a new draft-release being created as a result.

### Release workflow

Below are the steps of the release workflow. Steps prefixed with NOACTION are
automated and require no human action.

1. To initiate the release process, branch master off to a release branch and push it to Github:
  - `git checkout master; git pull; git checkout -b v0.8.12; git push origin refs/heads/v0.8.12`
2. NOACTION: The current HEAD of the release-candidate branch is tagged `v0.8.12-rc1`
3. NOACTION: A draft release and runtime WASMs are created for ths release-candidate
  automatically. A link to the draft release will be linked in the internal
  polkadot matrix channel (as well as an unsigned cleanroom-produced linux binary)
4. NOACTION: A new issue is created containing a checklist of manual steps to be completed before we are confident with the release.
5. Complete the steps in the issue created in step 4
6. (optional) If a fix is required to the release-candidate:
  1. Merge the fix with `master` first
  2. Checkout the release-candidate branch and merge upstream master
  3. Revert all changes since the creation of the release-candidate, excluding
    the fix.
  4. Push the release-candidate branch to Github
7. Once happy with the release-candidate, perform the release:
  1. Merge the release-candidate branch with the release branch and tag it:
    - `git checkout release`
    - `git pull`
    - `git merge v0.8.12`
    - `git tag -s -m 'v0.8.12' v0.8.12`
  2. Push the branch `git push origin release`
  3. NOACTION: The HEAD of the `release` branch will be tagged with `v0.8.12`, and a final release will be created and published on Github.
