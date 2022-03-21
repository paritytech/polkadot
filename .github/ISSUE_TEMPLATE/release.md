---
name: Release issue template
about: Tracking issue for new releases
title: Polkadot {{ env.VERSION }} Release checklist
---
# Release Checklist

This is the release checklist for Polkadot {{ env.VERSION }}. **All** following
checks should be completed before publishing a new release of the
Polkadot/Kusama/Westend/Rococo runtime or client. The current release candidate can be
checked out with `git checkout release-{{ env.VERSION }}`

### Runtime Releases

These checks should be performed on the codebase prior to forking to a release-
candidate branch.

- [ ] Verify [`spec_version`](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#spec-version) has been incremented since the
    last release for any native runtimes from any existing use on public
    (non-private) networks. If the runtime was published (release or pre-release), either
    the `spec_version` or `impl` must be bumped.
- [ ] Verify previously [completed migrations](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#old-migrations-removed) are
    removed for any public (non-private/test) networks.
- [ ] Verify pallet and [extrinsic ordering](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#extrinsic-ordering) has stayed
    the same. Bump `transaction_version` if not.
- [ ] Verify new extrinsics have been correctly whitelisted/blacklisted for
    [proxy filters](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#proxy-filtering).
- [ ] Verify [benchmarks](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#benchmarks) have been updated for any modified
    runtime logic.

The following checks can be performed after we have forked off to the release-
candidate branch or started an additional release candidate branch (rc-2, rc-3, etc)

- [ ] Verify [new migrations](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#new-migrations) complete successfully, and the
    runtime state is correctly updated for any public (non-private/test)
    networks.
- [ ] Verify [Polkadot JS API](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#polkadot-js) are up to date with the latest
    runtime changes.
- [ ] Check with the Signer's team to make sure metadata update QR are lined up
- [ ] Push runtime upgrade to Westend and verify network stability.

### All Releases

- [ ] Check that the new client versions have [run on the network](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#burn-in)
    without issue for 12+ hours on >75% of our validator nodes.
- [ ] Check that a draft release has been created at
    https://github.com/paritytech/polkadot/releases with relevant [release
    notes](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#release-notes)
- [ ] Check that [build artifacts](https://github.com/paritytech/polkadot/blob/master/doc/release-checklist.md#build-artifacts) have been added to the
    draft-release
- [ ] Check that all items listed in the [milestone](https://github.com/paritytech/polkadot/milestones) are included in the release.