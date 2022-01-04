# PVF Pre-checking

> ⚠️ This runtime API was added in v2.

There are two main runtime APIs to work with PVF pre-checking.

The first runtime API is designed to fetch all PVFs that require pre-checking voting. The PVFs are
identified by their code hashes. As soon as the PVF gains required support, the runtime API will
not return the PVF anymore.

```rust
fn pvfs_require_precheck() -> Vec<ValidationCodeHash>;
```

The second runtime API is needed to submit the judgement for a PVF, whether it is approved or not.
The voting process uses unsigned transactions. The [`PvfCheckStatement`](../types/pvf-prechecking.md) is circulated through the network via gossip similar to a normal transaction. At some point the validator
will include the statement in the block, where it will be processed by the runtime. If that was the
last vote before gaining the super-majority, this PVF will not be returned by `pvfs_require_precheck` anymore.

```rust
fn submit_pvf_check_statement(stmt: PvfCheckStatement, signature: ValidatorSignature);
```
