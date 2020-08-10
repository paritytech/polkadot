# Candidate Pending Availability

Get the receipt of a candidate pending availability. This returns `Some` for any paras assigned to occupied cores in `availability_cores` and `None` otherwise.

```rust
fn candidate_pending_availability(at: Block, ParaId) -> Option<CommittedCandidateReceipt>;
```
