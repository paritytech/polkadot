# Approval Voting

## Protocol

Input:
  - `ApprovalVotingMessage::CheckAndImportAssignment`
  - `ApprovalVotingMessage::CheckAndImportApproval`
  - `ApprovalVotingMessage::ApprovedAncestor`

Output:
  - `ApprovalNetworkingMessage::DistributeAssignment`
  - `ApprovalNetworkingMessage::DistributeApproval`
  - `RuntimeApiMessage::Request`
  - `ChainApiMessage`
  - `AvailabilityRecoveryMessage::Recover`
  - `CandidateExecutionMessage::ValidateFromExhaustive`

## Functionality

The approval voting subsystem is responsible for casting votes and determining approval of candidates and as a result, blocks.

This subsystem wraps a database which is used to store metadata about unfinalized blocks and the candidates within them. Candidates may appear in multiple blocks, but their approval status should be the same across all, due to the invariant that candidates must be included within the same session as their relay-parent.

## Database Schema

The database schema is designed with the following goals in mind:
  1. To provide an easy index from unfinalized blocks to candidates
  1. To provide a lookup from candidate hash to approval status
  1. To be easy to clear on start-up. What has happened while we were offline is unimportant.
  1. To be fast to clear entries outdated by finality

Structs:

```rust
struct TrancheEntry {
    tranche: DelayTranche,
    // assigned validators who have not yet approved, and the delay tranche
    // where we received their assignment.
    assignments: Vec<(ValidatorIndex, DelayTranche)>,
}

struct ApprovalEntry {
    tranches: Vec<TrancheEntry>, // sorted ascending by tranche number.
    backing_group: GroupIndex,
    approved: bool,
}

struct CandidateEntry {
    candidate: CandidateReceipt,
    session: SessionIndex,
    // Assignments are based on blocks, so we need to track assignments separately
    // based on the block we are looking at.
    block_assignments: HashMap<Hash, ApprovalEntry>,
    approvals: Vec<ValidatorIndex>,
}

struct BlockEntry {
    block_hash: Hash,
    session: SessionIndex,
    slot: SlotNumber,
    received_late_by: Duration,
    // random bytes derived from the VRF submitted within the block by the block
    // author as a credential and used as input to approval assignment criteria.
    relay_vrf_story: [u8; 32],
    // The candidates included as-of this block and the index of the core they are
    // leaving. Sorted ascending by core index.
    candidates: Vec<(CoreIndex, Hash)>,
    rotation_offset: GroupIndex,
    children: Vec<Hash>,
    approved: bool,
}

struct StoredBlockRange(BlockNumber, BlockNumber)
```

In the schema, we map

```
"StoredBlocks" => StoredBlockRange
BlockNumber => Vec<BlockHash>
BlockHash => BlockEntry
CandidateHash => CandidateEntry
```

## Logic

In-memory state:

```rust
struct State {
    earliest_session: SessionIndex,
    session_info: Vec<SessionInfo>,
    keystore: KeyStorePtr,
    // TODO: our own actively scheduled stuff.
}
```

[`SessionInfo`](../../runtime/session_info.md)

On start-up, we clear everything currently stored by the database. This is done by loading the `StoredBlockRange`, iterating through each block number, iterating through each block hash, and iterating through each candidate referenced by each block. Although this is O(n^3), we don't expect to have more than a few unfinalized blocks at any time and in extreme cases, a few thousand. The clearing operation should be relatively fast as a result.

On receiving an `OverseerSignal::BlockFinalized(h)`, we fetch the block number `b` of that block from the ChainApi subsystem. We update our `StoredBlockRange` to begin at `b+1`. Additionally, we remove all block entries and candidates referenced by them up to and including `b`. Lastly, we prune out all descendents of `h` transitively: when we remove a `BlockEntry` with number `b` that is not equal to `h`, we recursively delete all the `BlockEntry`s referenced as children. We remove the `block_assignments` entry for the block hash and if `block_assignments` is now empty, remove the `CandidateEntry`.

On receiving an `OverseerSignal::ActiveLeavesUpdate(update)`, we determine the set of new blocks that were not in our previous view. This is done by querying the ancestry of all new items in the view and contrasting against the stored `BlockNumber`s. Typically, there will be only one new block. We fetch the headers and information on these blocks from the ChainApi subsystem. We update the `StoredBlockRange` and the `BlockNumber` maps. We use the RuntimeApiSubsystem to determine the set of candidates included in these blocks and use BABE logic to determine the slot number and VRF of the blocks. We also note how late we appear to have received the block. We create a `BlockEntry` for each block and a `CandidateEntry` for each candidate. Ensure that the `CandidateEntry` contains a `block_assignments` entry for each candidate. 

On receiving a `ApprovalVotingMessage::CheckAndImportAssignment` message, we check the assignment cert against the block entry. The cert itself contains information necessary to determine the candidate that is being assigned-to. In detail:
  * Load the `BlockEntry` for the relay-parent referenced by the message.
  * Fetch the `SessionInfo` for the session of the block
  * Determine the assignment key of the validator based on that.
  * Check the assignment cert
    * If the cert kind is `RelayVRFModulo`, then the certificate is valid as long as `sample < session_info.relay_vrf_samples` and the VRF is valid for the validator's key with the input `block_entry.relay_vrf_story ++ sample.encode()` as described with [the approvals protocol section](../../protocol-approval.md#assignment-criteria). We set `core_index = vrf.make_bytes().to_u32() % session_info.n_cores`. If the `BlockEntry` causes inclusion of a candidate at `core_index`, then this is a valid assignment for the candidate at `core_index`. Otherwise, it can be ignored.
    * If the cert kind is `RelayVRFDelay`, then we check if the VRF is valid for the validator's key with the input `block_entry.relay_vrf_story ++ cert.core_index.encode()` as described in [the approvals protocol section](../../protocol-approval.md#assignment-criteria). The cert can be ignored if the block did not cause inclusion of a candidate on that core index. Otherwise, this is a valid assignment for the included candidate.