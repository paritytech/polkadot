# Candidates Included

This runtime API is for checking which candidates have been included within the chain, locally.

```rust
/// Input and output have the same length.
fn candidates_included(Vec<(SessionIndex, CandidateHash)>) -> Vec<bool>;
```
