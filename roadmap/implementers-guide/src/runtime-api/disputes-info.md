# Disputes Info

Get information about all disputes known by the chain as well as information about which validators the disputes subsystem will accept disputes from. These disputes may be either live or concluded. The [`DisputeState`](../types/disputes.md#disputestate) can be used to determine whether the dispute still accepts votes, as well as which validators' votes may be included.

```rust
struct Dispute {
    session: SessionIndex,
    candidate: CandidateHash,
    dispute_state: DisputeState,
    local: bool,
}

struct SpamSlotsInfo {
    max_spam_slots: u32,
    session_spam_slots: Vec<(SessionIndex, Vec<u32>)>,
}

struct DisputesInfo {
    disputes: Vec<Dispute>,
    spam_slots: SpamSlotsInfo,
}

fn disputes_info() -> DisputesInfo;
```
