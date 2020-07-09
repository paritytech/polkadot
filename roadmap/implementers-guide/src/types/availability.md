# Availability

One of the key roles of validators is to ensure availability of all data necessary to validate
candidates for the duration of a challenge period. This is done via an erasure-coding of the data to keep available.

## Signed Availability Bitfield

A bitfield [signed](backing.md#signed-wrapper) by a particular validator about the availability of pending candidates.


```rust
pub type SignedAvailabilityBitfield = Signed<Bitvec>;

struct Bitfields(Vec<(SignedAvailabilityBitfield)>), // bitfields sorted by validator index, ascending
```

## Proof-of-Validity

Often referred to as PoV, this is a type-safe wrapper around bytes (`Vec<u8>`) when referring to data that acts as a stateless-client proof of validity of a candidate, when used as input to the validation function of the para.

```rust
struct PoV(Vec<u8>);
```

## Omitted Validation Data

Validation data that is often omitted from types describing candidates as it can be derived from the relay-parent of the candidate. However, with the expectation of state pruning, these are best kept available elsewhere as well.

This contains the [`GlobalValidationSchedule`](candidate.md#globalvalidationschedule) and [`LocalValidationData`](candidate.md#localvalidationdata)

```rust
struct OmittedValidationData {
    /// The global validation schedule.
    pub global_validation: GlobalValidationSchedule,
    /// The local validation data.
    pub local_validation: LocalValidationData,
}
```


## Available Data

This is the data we want to keep available for each [candidate](candidate.md) included in the relay chain.

```rust
struct AvailableData {
	/// The Proof-of-Validation of the candidate.
	pub pov: PoV,
	/// The omitted validation data.
	pub omitted_validation: OmittedValidationData,
}
```

> TODO: With XCMP, we also need to keep available the outgoing messages as a result of para-validation.
