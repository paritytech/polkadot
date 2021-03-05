# Disputes

## DisputeStatementSet 

```rust
/// A set of statements about a specific candidate.
struct DisputeStatementSet {
    candidate_hash: CandidateHash,
    session: SessionIndex,
    statements: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
}
```

## DisputeStatement

```rust
/// A statement about a candidate, to be used within some dispute resolution process.
///
/// Statements are either in favor of the candidate's validity or against it.
enum DisputeStatement {
    /// A valid statement, of the given kind
    Valid(ValidDisputeStatementKind),
    Invalid(InvalidDisputeStatementKind),
}

```

## Dispute Statement Kinds

Kinds of dispute statements. Each of these can be combined with a candidate hash, session index, validator public key, and validator signature to reproduce and check the original statement.

```rust
enum ValidDisputeStatementKind {
    Explicit,
    BackingSeconded,
    BackingValid,
    ApprovalChecking,
}

enum InvalidDisputeStatementKind {
    Explicit,
}
```

## ExplicitDisputeStatement

```rust
struct ExplicitDisputeStatement {
    valid: bool,
    candidate_hash: CandidateHash,
    session: SessionIndex,
}
```

## MultiDisputeStatementSet

Sets of statements for many (zero or more) disputes.

```rust
type MultiDisputeStatementSet = Vec<DisputeStatementSet>;
```

## DisputeState

```rust
struct DisputeState {
    validators_for: Bitfield, // one bit per validator.
    validators_against: Bitfield, // one bit per validator.
    start: BlockNumber,
    concluded_at: Option<BlockNumber>,
}
```
