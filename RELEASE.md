# Release Checklist

The following checks should be completed before releasing a new version of the Polkadot/Kusama/Westend runtime or client:

- [] Verify new migrations complete successfully, and the runtime state is correctly updated.
- [] Verify previously completed migrations are removed. (`on_runtime_upgrade`)
- [] Verify pallet and extrinsic ordering has stayed the same. (check metadata)
- [] Verify new extrinsics have been correctly whitelisted/blacklisted for proxy filters.
- [] Check that the new client releases have run on the network without issue for 24 hours.
- [] Verify benchmarks have been updated for any modified runtime logic.
