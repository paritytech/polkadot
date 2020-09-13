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
    vrf_randomness: [u8; 32],
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
    // TODO: our own actively scheduled stuff.
}
```

On start-up, we clear everything currently stored by the database. This is done by loading the `StoredBlockRange`, iterating through each block number, iterating through each block hash, and iterating through each candidate referenced by each block. Although this is O(n^3), we don't expect to have more than a few unfinalized blocks at any time and in extreme cases, a few thousand. The clearing operation should be relatively fast as a result.

On receiving an `OverseerSignal::BlockFinalized(h)`, we fetch the block number `b` of that block from the ChainApi subsystem. We update our `StoredBlockRange` to begin at `b+1`. Additionally, we remove all block entries and candidates referenced by them up to and including `b`. Lastly, we prune out all descendents of `h` transitively: when we remove a `BlockEntry` with number `b` that is not equal to `h`, we recursively delete all the `BlockEntry`s referenced as children. We remove the `block_assignments` entry for the block hash and if `block_assignments` is now empty, remove the `CandidateEntry`.

On receiving an `OverseerSignal::ActiveLeavesUpdate(update)`, we determine the set of new blocks that were not in our previous view. This is done by querying the ancestry of all new items in the view and contrasting against the stored `BlockNumber`s. Typically, there will be only one new block. We fetch the headers and information on these blocks from the ChainApi subsystem. We update the `StoredBlockRange` and the `BlockNumber` maps. We use the RuntimeApiSubsystem to determine the set of candidates included in these blocks and use BABE logic to determine the slot number and VRF of the blocks. We create a `BlockEntry` for each block and a `CandidateEntry` for each candidate. Ensure that the `CandidateEntry` contains a `block_assignments` entry for each candidate.

On receiving a `CheckAndImportAssignment` message, we check the assignment cert against the block entry. 