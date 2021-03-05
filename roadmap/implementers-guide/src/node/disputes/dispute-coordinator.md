# Dispute Coordinator

This is the central subsystem of the node-side components which participate in disputes. This subsystem wraps a database which tracks all statements observed by all validators over some window of sessions. Votes older than this session window are pruned.

This subsystem will be the point which produce dispute votes, eiuther positive or negative, based on locally-observed validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from another node, this will trigger the [dispute participation](dispute-participation.md) subsystem to recover and validate the block and call back to this subsystem.

## Database Schema

We use an underlying Key-Value database where we assume we have the following operations available:
  * `write(key, value)`
  * `read(key) -> Option<value>`
  * `iter_with_prefix(prefix) -> Iterator<(key, value)>` - gives all keys and values in lexicographical order where the key starts with `prefix`.

We use this database to encode the following schema:

```rust
(SessionIndex, "candidate-votes", CandidateHash) -> Option<CandidateVotes>
```

The meta information that we track per-candidate is defined as the `CandidateVotes` struct.
This draws on the [dispute statement types](../../types/disputes.md)

```rust
struct CandidateVotes {
    positive: Vec<(ValidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
    negative: Vec<(InvalidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
}
```

## Protocol

Input: [`DisputeCoordinatorMessage`](../../types/overseer-protocol.md) TODO

Output: TODO
