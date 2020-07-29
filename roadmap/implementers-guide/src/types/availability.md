# Availability

One of the key roles of validators is to ensure availability of all data necessary to validate
candidates for the duration of a challenge period. This is done via an erasure-coding of the data to keep available.

## Signed Availability Bitfield

A bitfield [signed](backing.md#signed-wrapper) by a particular validator about the availability of pending candidates.


```rust
type SignedAvailabilityBitfield = Signed<Bitvec>;

struct Bitfields(Vec<(SignedAvailabilityBitfield)>), // bitfields sorted by validator index, ascending
```

### Semantics

A `SignedAvailabilityBitfield` represents the view from a particular validator's perspective. Each bit in the bitfield corresponds to a single [availability core](../runtime-api/availability-cores.md). A `1` bit indicates that the validator believes the following statements to be true for a core:

- the availability core is occupied
- there exists a [`CommittedCandidateReceipt`](candidate.html#committed-candidate-receipt) corresponding to that core. In other words, that para has a block in progress.
- the validator's [Availability Store](../node/utility/availability-store.md) contains a chunk of that parablock's PoV.

In other words, it is the transpose of [`OccupiedCore::availability`](../runtime-api/availability-cores.md).

## Proof-of-Validity

Often referred to as PoV, this is a type-safe wrapper around bytes (`Vec<u8>`) when referring to data that acts as a stateless-client proof of validity of a candidate, when used as input to the validation function of the para.

```rust
struct PoV(Vec<u8>);
```

## Omitted Validation Data

Validation data that is often omitted from types describing candidates as it can be derived from the relay-parent of the candidate. However, with the expectation of state pruning, these are best kept available elsewhere as well.

This contains the [`GlobalValidationData`](candidate.md#globalvalidationschedule) and [`LocalValidationData`](candidate.md#localvalidationdata)

```rust
struct OmittedValidationData {
    /// The global validation schedule.
    global_validation: GlobalValidationData,
    /// The local validation data.
    local_validation: LocalValidationData,
}
```


## Available Data

This is the data we want to keep available for each [candidate](candidate.md) included in the relay chain.

```rust
struct AvailableData {
    /// The Proof-of-Validation of the candidate.
    pov: PoV,
    /// The omitted validation data.
    omitted_validation: OmittedValidationData,
}
```

> TODO: With XCMP, we also need to keep available the outgoing messages as a result of para-validation.

## Erasure Chunk

The [`AvailableData`](#availabledata) is split up into an erasure-coding as part of the availability process. Each validator gets a chunk. This describes one of those chunks, along with its proof against a merkle root hash, which should be apparent from context, and is the `erasure_root` field of a [`CandidateDescriptor`](candidate.md#candidatedescriptor).


```rust
struct ErasureChunk {
    /// The erasure-encoded chunk of data belonging to the candidate block.
    chunk: Vec<u8>,
    /// The index of this erasure-encoded chunk of data.
    index: u32,
    /// Proof for this chunk's branch in the Merkle tree.
    proof: Vec<Vec<u8>>,
}
```
