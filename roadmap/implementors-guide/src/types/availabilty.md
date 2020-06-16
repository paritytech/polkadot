# Availability

One of the key roles of validators is to ensure availability of all data necessary to validate
candidates for the duration of a challenge period. This is done via an erasure-coding of the data to keep available.

## Signed Availability Bitfield

A bitfield signed by a particular validator about the availability of pending candidates.

```rust
struct SignedAvailabilityBitfield {
  validator_index: ValidatorIndex,
  bitfield: Bitvec,
  signature: ValidatorSignature, // signature is on payload: bitfield ++ relay_parent ++ validator index
}

struct Bitfields(Vec<(SignedAvailabilityBitfield)>), // bitfields sorted by validator index, ascending
```

## Proof-of-Validity

Often referred to as PoV, this is a type-safe wrapper around bytes (`Vec<u8>`) when referring to data that acts as a stateless-client proof of validity of a candidate, when used as input to the validation function of the para.
