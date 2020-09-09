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

```rust
struct ApprovalVote(ExecutionTimePair);
```

## SignedApprovalVote

```rust
struct SignedApprovalVote {
    vote: ApprovalVote,
    validator: ValidatorIndex,
    signature: ApprovalSignature,
}
```

## RelayVRFStory

Assignment criteria are based off of possible stories about the relay-chain block that included the candidate. More information on stories is available in [the informational page on approvals.](../approval.md#stories).

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

## SubmittedAssignments

These are assignments submitted to the relay chain. Assignments may be submitted for any candidate that is pending approval. The assignments are ordered first by block and then by candidate.

```rust
// Assignments submitted for a particular core. In the context of a block.
struct SubmittedCoreAssignments {
    core: CoreIndex,
    // ordered by validator index.
    // validator index is within the session where the block was included.
    assigned: Vec<(ValidatorIndex, AssignmentCert, VRFInOut)>,
}

// Ordered ascending by block number and the per-candidate assignments are ordered by
// core index.
type SubmittedAssignments = Vec<(BlockNumber, Vec<SubmittedCoreAssignments>)>;
```

## SubmittedApprovals

Approval votes submitted to the chain.

```rust
// Approvals submitted for a particular core. In the context of a block.
struct SubmittedCoreApprovals {
    core: CoreIndex,
    // ordered by validator index.
    // validator index is within the sesison where the block was included.
    approvals: Vec<(ValidatorIndex, SignedApprovalVote)>,
}

// Sorted ascending by `BlockNumber`. A `bool` value of `true` indicates that the block
// has had all candidates approved.
type SubmittedApprovals = Vec<(BlockNumber, Vec<SubmittedCoreApprovals>, bool)>;
```