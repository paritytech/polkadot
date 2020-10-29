---
name: Release issue template
about: Tracking issue for new releases
title: Polkadot {{ env.VERSION }} Release checklist
---
# Release Checklist

This is the release checklist for Polkadot {{ env.VERSION }}. **All** following
checks should be completed before publishing a new release of the
Polkadot/Kusama/Westend runtime or client. The current release candidate can be
checked out with `git checkout {{ env.VERSION }}`

### Runtime Releases

- [ ] Verify [`spec_version`](#spec-version) has been incremented since the
    last release for any native runtimes from any existing use on public
    (non-private/test) networks.
- [ ] Verify [new migrations](#new-migrations) complete successfully, and the
    runtime state is correctly updated.
- [ ] Verify previously [completed migrations](#old-migrations-removed) are
    removed.
- [ ] Verify pallet and [extrinsic ordering](#extrinsic-ordering) has stayed
    the same. Bump `transaction_version` if not.
- [ ] Verify new extrinsics have been correctly whitelisted/blacklisted for
    [proxy filters](#proxy-filtering).
- [ ] Verify [benchmarks](#benchmarks) have been updated for any modified
    runtime logic.
- [ ] Verify [Polkadot JS API](#polkadot-js) are up to date with the latest
    runtime changes.

### All Releases

- [ ] Check that the new client versions have [run on the network](#burn-in)
    without issue for 12 hours.
- [ ] Check that a draft release has been created at
    https://github.com/paritytech/polkadot/releases with relevant [release
    notes](#release-notes)
- [ ] Check that [build artifacts](#build-artifacts) have been added to the
    draft-release

## Notes

### Burn In

Ensure that Parity DevOps has run the new release on Westend, Kusama, and
Polkadot validators for at least 12 hours prior to publishing the release.

### Build Artifacts

Add any necessary assets to the release. They should include:

- Linux binary
- GPG signature of the Linux binary
- SHA256 of binary
- Source code
- Wasm binaries of any runtimes

### Release notes

The release notes should list:

- The priority of the release (i.e., how quickly users should upgrade)
- Which native runtimes and their versions are included
- The proposal hashes of the runtimes as built with
    [srtool](https://gitlab.com/chevdor/srtool)
- Any changes in this release that are still awaiting audit

The release notes may also list:

- Free text at the beginning of the notes mentioning anything important
    regarding this release
- Notable changes (those labelled with B[1-9]-* labels) separated into sections

### Spec Version

A runtime upgrade must bump the spec number. This may follow a pattern with the
client release (e.g. runtime v12 corresponds to v0.8.12, even if the current
runtime is not v11).

### New Migrations

Ensure that any migrations that are required due to storage or logic changes
are included in the `on_runtime_upgrade` function of the appropriate pallets.

### Old Migrations Removed

Any previous `on_runtime_upgrade` functions from old upgrades must be removed
to prevent them from executing a second time.

### Extrinsic Ordering

Offline signing libraries depend on a consistent ordering of call indices and
functions. Compare the metadata of the current and new runtimes and ensure that
the `module index, call index` tuples map to the same set of functions. In case
of a breaking change, increase `transaction_version`.

Note: Adding new functions to the runtime does not constitute a breaking change
as long as they are added to the end of a pallet (i.e., does not break any
other call index).

### Proxy Filtering

The runtime contains proxy filters that map proxy types to allowable calls. If
the new runtime contains any new calls, verify that the proxy filters are up to
date to include them.

### Benchmarks

Run the benchmarking suite with the new runtime and update any function weights
if necessary.

### Polkadot JS

Ensure that a release of [Polkadot JS API]() contains any new types or
interfaces necessary to interact with the new runtime.
