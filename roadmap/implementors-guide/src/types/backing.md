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

## Signed Wrapper

There are a few distinct types which we desire to sign, and validate the signatures of. Instead of duplicating this work, we extract a signed wrapper.

```rust,ignore
struct Signed<Payload, RealPayload=Payload> {
    /// The payload is part of the signed data. The rest is the signing context,
    /// which is known both at signing and at validation.
    ///
    /// Note that this field is not publicly accessible: this reduces the chances
    /// of accidentally invalidating the signature by mutating the payload.
    payload: Payload,
    /// The index of the validator signing this statement.
    pub validator_index: ValidatorIndex,
    /// The signature by the validator of the signed payload.
    pub signature: ValidatorSignature,
}

impl<Payload: Encode + Into<RealPayload>, RealPayload> Signed<Payload, RealPayload> {
    fn sign(payload: Payload, context: SigningContext, index: ValidatorIndex, key: PrivateKey) -> Signed<Payload, RealPayload> { ... }
    fn validate(&self, context: SigningContext, key: PublicKey) -> bool { ... }
    fn payload(&self) -> &Payload { &self.payload }
}
```

Note the presence of the [`SigningContext`](../types/candidate.html#signing-context) in the signatures of the `sign` and `validate` methods. To ensure cryptographic security, the actual signed payload is always the SCALE encoding of `(payload.into(), signing_context)`. Including the signing context prevents replay attacks. Converting the `T` into `U` is a NOP most of the time, but it allows us to substitute `CompactStatement`s for `Statement`s on demand, which helps efficiency.

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

/// A statement about the validity of a parachain candidate.
///
/// This variant should only be used in the production of `SignedStatement`s. The only difference between
/// this enum and `Statement` is that the `Seconded` variant contains a `Hash` instead of a `CandidateReceipt`.
/// The rationale behind the difference is that a `CandidateReceipt` contains `HeadData`, which does not have
/// bounded size. By using this enum instead, we ensure that the production and validation of signatures is fast
/// while retaining their necessary cryptographic properties.
enum CompactStatement {
  /// A statement about a new candidate being seconded by a validator. This is an implicit validity vote.
  Seconded(Hash),
  /// A statement about the validity of a candidate, based on candidate's hash.
  Valid(Hash),
  /// A statement about the invalidity of a candidate.
  Invalid(Hash),
}
```

`CompactStatement` exists because a `CandidateReceipt` includes `HeadData`, which does not have a bounded size.

## Signed Statement Type

A statement which has been [cryptographically signed](#signed-wrapper) by a validator.

```rust
/// A signed statement.
pub type SignedStatement = Signed<Statement, CompactStatement>;
```

Munging the signed `Statement` into a `CompactStatement` before signing allows the candidate receipt itself to be omitted when checking a signature on a `Seconded` statement.

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
