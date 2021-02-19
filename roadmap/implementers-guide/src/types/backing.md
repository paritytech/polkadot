# Backing Types

[Candidates](candidate.md) go through many phases before being considered included in a fork of the relay chain and eventually accepted.

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

## Signed Wrapper

There are a few distinct types which we desire to sign, and validate the signatures of. Instead of duplicating this work, we extract a signed wrapper.

```rust,ignore
/// A signed type which encapsulates the common desire to sign some data and validate a signature.
///
/// Note that the internal fields are not public; they are all accessable by immutable getters.
/// This reduces the chance that they are accidentally mutated, invalidating the signature.
struct Signed<Payload, RealPayload=Payload> {
    /// The payload is part of the signed data. The rest is the signing context,
    /// which is known both at signing and at validation.
    payload: Payload,
    /// The index of the validator signing this statement.
    validator_index: ValidatorIndex,
    /// The signature by the validator of the signed payload.
    signature: ValidatorSignature,
}

impl<Payload: EncodeAs<RealPayload>, RealPayload: Encode> Signed<Payload, RealPayload> {
    fn sign(payload: Payload, context: SigningContext, index: ValidatorIndex, key: ValidatorPair) -> Signed<Payload, RealPayload> { ... }
    fn validate(&self, context: SigningContext, key: ValidatorId) -> bool { ... }
}
```

Note the presence of the [`SigningContext`](../types/candidate.md#signing-context) in the signatures of the `sign` and `validate` methods. To ensure cryptographic security, the actual signed payload is always the SCALE encoding of `(payload.into(), signing_context)`. Including the signing context prevents replay attacks.

`EncodeAs` is a helper trait with a blanket impl which ensures that any `T` can `EncodeAs<T>`. Therefore, for the generic case where `RealPayload = Payload`, it changes nothing. However, we  `impl EncodeAs<CompactStatement> for Statement`, which helps efficiency.

## Statement Type

The [Candidate Backing subsystem](../node/backing/candidate-backing.md) issues and signs these after candidate validation.

```rust
/// A statement about the validity of a parachain candidate.
enum Statement {
  /// A statement about a new candidate being seconded by a validator. This is an implicit validity vote.
  ///
  /// The main semantic difference between `Seconded` and `Valid` comes from the fact that every validator may
  /// second only 1 candidate; this places an upper bound on the total number of candidates whose validity
  /// needs to be checked. A validator who seconds more than 1 parachain candidate per relay head is subject
  /// to slashing.
  Seconded(CommittedCandidateReceipt),
  /// A statement about the validity of a candidate, based on candidate's hash.
  Valid(Hash),
}

/// A statement about the validity of a parachain candidate.
///
/// This variant should only be used in the production of `SignedStatement`s. The only difference between
/// this enum and `Statement` is that the `Seconded` variant contains a `Hash` instead of a `CandidateReceipt`.
/// The rationale behind the difference is that the signature should always be on the hash instead of the
/// full data, as this lowers the requirement for checking while retaining necessary cryptographic properties
enum CompactStatement {
  /// A statement about a new candidate being seconded by a validator. This is an implicit validity vote.
  Seconded(Hash),
  /// A statement about the validity of a candidate, based on candidate's hash.
  Valid(Hash),
}
```

`CompactStatement` exists because a `CandidateReceipt` includes `HeadData`, which does not have a bounded size.

## Signed Statement Type

A statement which has been [cryptographically signed](#signed-wrapper) by a validator.

```rust
/// A signed statement, containing the committed candidate receipt in the `Seconded` variant.
pub type SignedFullStatement = Signed<Statement, CompactStatement>;

/// A signed statement, containing only the hash.
pub type SignedStatement = Signed<CompactStatement>;
```

Munging the signed `Statement` into a `CompactStatement` before signing allows the candidate receipt itself to be omitted when checking a signature on a `Seconded` statement.

## Backed Candidate

An [`CommittedCandidateReceipt`](candidate.md#committed-candidate-receipt) along with all data necessary to prove its backing. This is submitted to the relay-chain to process and move along the candidate to the pending-availability stage.

```rust
struct BackedCandidate {
  candidate: CommittedCandidateReceipt,
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
