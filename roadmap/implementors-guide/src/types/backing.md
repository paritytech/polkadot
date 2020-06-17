# Backing Types

[Candidates](candidate.html) go through many phases before being considered included in a fork of the relay chain and eventually accepted.

These types describe the data used in the backing phase. Some are sent over the wire within subsystems, and some are simply included in the relay-chain block.

## Validity Attestation

An attestation of validity for a candidate, used as part of a backing. Both the `Seconded` and `Valid` statements are considered attestations of validity. This structure is only useful where the candidate referenced is apparent.

```rust
enum ValidityAttestation {
  /// Implicit validity attestation by issuing.
  /// This corresponds to issuance of a `Seconded` statement.
  Implicit(ValidatorSignature),
  /// An explicit attestation. This corresponds to issuance of a
  /// `Valid` statement.
  Explicit(ValidatorSignature),
}
```

## Statement Type

The [Candidate Backing subsystem](../node/backing/candidate-backing.html) issues and signs these after candidate validation.

```rust
/// A statement about the validity of a parachain candidate.
enum Statement {
  /// A statement about a new candidate being seconded by a validator. This is an implicit validity vote.
  ///
  /// The main semantic difference between `Seconded` and `Valid` comes from the fact that every validator may
  /// second only 1 candidate; this places an upper bound on the total number of candidates whose validity
  /// needs to be checked. A validator who seconds more than 1 parachain candidate per relay head is subject
  /// to slashing.
  Seconded(CandidateReceipt),
  /// A statement about the validity of a candidate, based on candidate's hash.
  Valid(Hash),
  /// A statement about the invalidity of a candidate.
  Invalid(Hash),
}
```

## Signed Statement Type

A statement, the identifier of a validator, and a signature.

```rust
/// A signed statement.
struct SignedStatement {
  /// The index of the validator signing this statement.
  validator_index: ValidatorIndex,
  /// The statement itself.
  statement: Statement,
  /// The signature by the validator on the signing payload.
  signature: ValidatorSignature
}
```

The actual signed payload will be the SCALE encoding of `(compact_statement, signing_context)` where
`compact_statement` is a tweak of the [`Statement`](#statement) enum where all variants, including `Seconded`, contain only the hash of the candidate, and the `signing_context` is a [`SigningContext`](../types/candidate.html#signing-context).

This prevents against replay attacks and allows the candidate receipt itself to be omitted when checking a signature on a `Seconded` statement in situations where the hash is known.

## Backed Candidate

An [`AbridgedCandidateReceipt`](candidate.html#abridgedcandidatereceipt) along with all data necessary to prove its backing. This is submitted to the relay-chain to process and move along the candidate to the pending-availability stage.

```rust
struct BackedCandidate {
  candidate: AbridgedCandidateReceipt,
  validity_votes: Vec<ValidityAttestation>,
  // the indices of validators who signed the candidate within the group. There is no need to include
  // bit for any validators who are not in the group, so this is more compact.
  // The number of bits is the number of validators in the group.
  //
  // the group should be apparent from context.
  validator_indices: BitVec,
}

struct BackedCandidates(Vec<BackedCandidate>); // sorted by para-id.
```
