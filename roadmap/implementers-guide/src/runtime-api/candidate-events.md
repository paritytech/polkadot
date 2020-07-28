# Candidate Events

Yields a vector of events concerning candidates that occurred within the given block.

```rust
enum CandidateEvent {
	/// This candidate receipt was backed in the most recent block.
	CandidateBacked(CandidateReceipt, HeadData),
	/// This candidate receipt was included and became a parablock at the most recent block.
	CandidateIncluded(CandidateReceipt, HeadData),
	/// This candidate receipt was not made available in time and timed out.
	CandidateTimedOut(CandidateReceipt, HeadData),
}

fn candidate_events(at: Block) -> Vec<CandidateEvent>;
```
