# Approval Types

## ApprovalId

The public key of a keypair used by a validator for approval voting.

## AssignmentId

The private key of a keypair used by a validator for approval voting.

## AssignmentCert

An `AssignmentCert`, short for Assignment Certificate, is a piece of data provided by a validator to prove that they have been selected to perform secondary approval checks on an included candidate.

These certificates can be checked in the context of a specific block, candidate, and validator assignment VRF key. The block state will also provide further context about the availability core states at that block.

```rust
enum AssignmentCertKind {
    RelayVRFModulo {
        relay_vrf: VRFInOut,
        sample: u32,
    },
    RelayVRFDelay {
        relay_vrf: VRFInOut,
    }
}

struct AssignmentCert {
    // The criterion which is claimed to be met by this cert.
    kind: AssignmentCertKind,
    // The VRF showing the criterion is met.
    vrf: VRFInOut,
}
```

> TODO: RelayEquivocation cert. Probably can only be broadcast to chains that have handled an equivocation report.

## ApprovalVote

A vote of approval on a candidate.

```rust
struct ApprovalVote(Hash, ExecutionTimePair);
```

## SignedApprovalVote

```rust
struct SignedApprovalVote {
    vote: ApprovalVote,
    validator: ValidatorIndex,
    signature: ApprovalSignature,
}
```

## CheckedAssignmentCert

An assignment cert which has been completely checked. This type should be declared in such a way as to be instantiable only when the checks have actually been done. Fields should be accessible via getters, not direct struct access.

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
type DelayTranche = u16;
```

## RelayVRFStory

Assignment criteria are based off of possible stories about the relay-chain block that included the candidate. More information on stories is available in [the informational page on approvals.](../protocol-approval.md#stories).

```rust
/// A story based on the VRF that authorized the relay-chain block where the candidate was
/// included.
///
/// VRF Context is "A&V RC-VRF"
struct RelayVRFStory(VRFInOut);
```

## RelayEquivocationStory

```rust
/// A story based on the candidate hash itself. Should be used when a candidate is an
/// equivocation: when there are two relay-chain blocks with the same RelayVRFStory, but only
/// one contains the candidate.
///
/// VRF Context is "A&V RC-EQUIV"
struct RelayEquivocationStory(Hash);
```

## ExecutionTimePair

```rust
struct ExecutionTimePair {
    // The absolute time in milliseconds that the validator claims to have taken
    // with the block.
    absolute: u32,
    // The validator's believed ratio in execution time to the average, expressed as a fixed-point
    // 16-bit unsigned integer with 8 bits before and after the point.
    ratio: FixedU16,
}
```