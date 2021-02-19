# Known Disputes

Get all disputes known by the chain. These disputes may be either live or concluded. The [`DisputeState`](../types/disputes.md#disputestate) can be used to determine whether the dispute still accepts votes, as well as which validators' votes may be included.

```rust
fn known_disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState)>;
```
