Polkadot Release Process
------------------------

### Branches
* release-candidate branch: The branch used for staging of the next release.
  Named like `release-v0.8.26` 
* release branch: The branch to which successful release-candidates are merged
  and tagged with the new version. Named literally `release`.

### Notes
* The release-candidate branch *must* be made in the paritytech/polkadot repo in
order for release automation to work correctly
* Any new pushes/merges to the release-candidate branch (for example, 
refs/heads/release-v0.8.26) will result in the rc index being bumped (e.g., v0.8.26-rc1
to v0.8.26-rc2) and new wasms built.

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
  2. Checkout the release-candidate branch and merge `master`
  3. Revert all changes since the creation of the release-candidate that are
  **not** required for the fix.
  4. Push the release-candidate branch to Github - this is now the new release-
  candidate
7. Once happy with the release-candidate, perform the release using the release
  script located at `scripts/release.sh` (or perform the steps in that script
  manually):
  - `./scripts/release.sh v0.8.26`
8. NOACTION: The HEAD of the `release` branch will be tagged with `v0.8.26`,
  and a final release will be created on Github.