# Approval Types

## AssignmentId

The public key of a keypair used by a validator for determining assignments to approve included parachain candidates.

## AssignmentCert

An `AssignmentCert`, short for Assignment Certificate, is a piece of data provided by a validator to prove that they have been selected to perform secondary approval checks on an included candidate.

These certificates can be checked in the context of a specific block, candidate, and validator assignment VRF key. The block state will also provide further context about the availability core states at that block.

```rust
enum AssignmentCertKind {
    RelayVRFModulo {
        sample: u32,
    },
    RelayVRFDelay {
        core_index: CoreIndex,
    }
}

struct AssignmentCert {
    // The criterion which is claimed to be met by this cert.
    kind: AssignmentCertKind,
    // The VRF showing the criterion is met.
    vrf: (VRFPreOut, VRFProof),
}
```

> TODO: RelayEquivocation cert. Probably can only be broadcast to chains that have handled an equivocation report.

## IndirectAssignmentCert

An assignment cert which refers to the candidate under which the assignment is relevant by block hash.

```rust
struct IndirectAssignmentCert {
    // A block hash where the candidate appears.
    block_hash: Hash,
    validator: ValidatorIndex,
    cert: AssignmentCert,
}
```

## ApprovalVote

A vote of approval on a candidate.

```rust
struct ApprovalVote(Hash);
```

## SignedApprovalVote

An approval vote signed with a validator's key. This should be verifiable under the `ValidatorId` corresponding to the `ValidatorIndex` of the session, which should be implicit from context.

```rust
struct SignedApprovalVote {
    vote: ApprovalVote,
    validator: ValidatorIndex,
    signature: ValidatorSignature,
}
```

## IndirectSignedApprovalVote

A signed approval vote which references the candidate indirectly via the block. If there exists a look-up to the candidate hash from the block hash and candidate index, then this can be transformed into a `SignedApprovalVote`.

Although this vote references the candidate by a specific block hash and candidate index, the signature is computed on the actual `SignedApprovalVote` payload.

```rust
struct IndirectSignedApprovalVote {
    // A block hash where the candidate appears.
    block_hash: Hash,
    // The index of the candidate in the list of candidates fully included as-of the block.
    candidate_index: CandidateIndex,
    validator: ValidatorIndex,
    signature: ValidatorSignature,
}
```

## CheckedAssignmentCert

An assignment cert which has checked both the VRF and the validity of the implied assignment according to the selection criteria rules of the protocol. This type should be declared in such a way as to be instantiable only when the checks have actually been done. Fields should be accessible via getters, not direct struct access.

```rust
struct CheckedAssignmentCert {
    cert: AssignmentCert,
    validator: ValidatorIndex,
    relay_block: Hash,
    candidate_hash: Hash,
    delay_tranche: DelayTranche,
}
```

## DelayTranche

```rust
type DelayTranche = u32;
```
