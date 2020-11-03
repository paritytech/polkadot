# Approval Voting

Reading the [section on the approval protocol](../../protocol-approval.md) will likely be necessary to understand the aims of this subsystem.

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

This subsystem wraps a database which is used to store metadata about unfinalized blocks and the candidates within them. Candidates may appear in multiple blocks, and assignment criteria are chosen differently based on the hash of the block they appear in.

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
    // assigned validators who have not yet approved, and the instant we received
    // their assignment.
    assignments: Vec<(ValidatorIndex, Tick)>,
}

struct OurAssignment {
  cert: AssignmentCert,
  tranche: DelayTranche,
  validator_index: ValidatorIndex,
  triggered: bool,
}

struct ApprovalEntry {
    tranches: Vec<TrancheEntry>, // sorted ascending by tranche number.
    backing_group: GroupIndex,
    // When the next wakeup for this entry should occur. This is either to
    // check a no-show or to check if we need to broadcast an assignment.
    next_wakeup: Tick,
    our_assignment: Option<OurAssignment>,
    assignments: Bitfield, // n_validators bits
    approved: bool,
}

struct CandidateEntry {
    candidate: CandidateReceipt,
    session: SessionIndex,
    // Assignments are based on blocks, so we need to track assignments separately
    // based on the block we are looking at.
    block_assignments: HashMap<Hash, ApprovalEntry>,
    approvals: Bitfield, // n_validators bits
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
    // A bitfield where the i'th bit corresponds to the i'th candidate in `candidates`.
    // The i'th bit is `true` iff the candidate has been approved in the context of
    // this block. The block can be considered approved has all bits set to 1
    approved_bitfield: Bitfield,
    rotation_offset: GroupIndex,
    children: Vec<Hash>,
}

// slot_duration * 2 + DelayTranche gives the number of delay tranches since the
// unix epoch.
type Tick = u64;

struct TrackerEntry 

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
struct ApprovalVoteRequest {
  validator_index: ValidatorIndex,
  block_hash: Hash,
  candidate_index: u32,
}

struct State {
    earliest_session: SessionIndex,
    session_info: Vec<SessionInfo>,
    keystore: KeyStorePtr,
    wakeups: BTreeMap<Tick, Vec<(Hash, Hash)>>, // Tick -> [(Relay Block, Candidate Hash)]
    
    // These are connected to each other.
    approval_vote_tx: mpsc::Sender<ApprovalVoteRequest>,
    approval_vote_rx: mpsc::Receiver<ApprovalVoteRequest>,
}
```

[`SessionInfo`](../../runtime/session_info.md)

On start-up, we clear everything currently stored by the database. This is done by loading the `StoredBlockRange`, iterating through each block number, iterating through each block hash, and iterating through each candidate referenced by each block. Although this is `O(o*n*p)`, we don't expect to have more than a few unfinalized blocks at any time and in extreme cases, a few thousand. The clearing operation should be relatively fast as a result.

Main loop:
  * Each iteration, select over all of
    * The next `Tick` in `wakeups`: trigger `wakeup_process` for each `(Hash, Hash)` pair scheduled under the `Tick` and then remove all entries under the `Tick`.
    * The next message from the overseer: handle the message as described in the [Incoming Messages section](#incoming-messages)
    * The next request from `approval_vote_rx`: handle with `issue_approval`

### Incoming Messages

#### `OverseerSignal::BlockFinalized`

On receiving an `OverseerSignal::BlockFinalized(h)`, we fetch the block number `b` of that block from the ChainApi subsystem. We update our `StoredBlockRange` to begin at `b+1`. Additionally, we remove all block entries and candidates referenced by them up to and including `b`. Lastly, we prune out all descendents of `h` transitively: when we remove a `BlockEntry` with number `b` that is not equal to `h`, we recursively delete all the `BlockEntry`s referenced as children. We remove the `block_assignments` entry for the block hash and if `block_assignments` is now empty, remove the `CandidateEntry`.


#### `OverseerSignal::ActiveLeavesUpdate`

On receiving an `OverseerSignal::ActiveLeavesUpdate(update)`:
  * We determine the set of new blocks that were not in our previous view. This is done by querying the ancestry of all new items in the view and contrasting against the stored `BlockNumber`s. Typically, there will be only one new block. We fetch the headers and information on these blocks from the ChainApi subsystem. 
  * We update the `StoredBlockRange` and the `BlockNumber` maps. We use the RuntimeApiSubsystem to determine the set of candidates included in these blocks and use BABE logic to determine the slot number and VRF of the blocks. 
  * We also note how late we appear to have received the block. We create a `BlockEntry` for each block and a `CandidateEntry` for each candidate obtained from `CandidateIncluded` events after making a `RuntimeApiRequest::CandidateEvents` request.
  * Ensure that the `CandidateEntry` contains a `block_assignments` entry for the block, with the correct backing group set.
  * If a validator in this session, compute and assign `our_assignment` for the `block_assignments`
    * Only if not a member of the backing group.
    * Run `RelayVRFModulo` and `RelayVRFDelay` according to the [the approvals protocol section](../../protocol-approval.md#assignment-criteria)
  * invoke `process_wakeup(relay_block, candidate)` for each new candidate in each new block - this will automatically broadcast a 0-tranche assignment, kick off approval work, and schedule the next delay.

#### `ApprovalVotingMessage::CheckAndImportAssignment`

On receiving a `ApprovalVotingMessage::CheckAndImportAssignment` message, we check the assignment cert against the block entry. The cert itself contains information necessary to determine the candidate that is being assigned-to. In detail:
  * Load the `BlockEntry` for the relay-parent referenced by the message. If there is none, return `VoteCheckResult::Report`.
  * Fetch the `SessionInfo` for the session of the block
  * Determine the assignment key of the validator based on that.
  * Check the assignment cert
    * If the cert kind is `RelayVRFModulo`, then the certificate is valid as long as `sample < session_info.relay_vrf_samples` and the VRF is valid for the validator's key with the input `block_entry.relay_vrf_story ++ sample.encode()` as described with [the approvals protocol section](../../protocol-approval.md#assignment-criteria). We set `core_index = vrf.make_bytes().to_u32() % session_info.n_cores`. If the `BlockEntry` causes inclusion of a candidate at `core_index`, then this is a valid assignment for the candidate at `core_index` and has delay tranche 0. Otherwise, it can be ignored.
    * If the cert kind is `RelayVRFDelay`, then we check if the VRF is valid for the validator's key with the input `block_entry.relay_vrf_story ++ cert.core_index.encode()` as described in [the approvals protocol section](../../protocol-approval.md#assignment-criteria). The cert can be ignored if the block did not cause inclusion of a candidate on that core index. Otherwise, this is a valid assignment for the included candidate. The delay tranche for the assignment is determined by reducing `(vrf.make_bytes().to_u64() % (session_info.n_delay_tranches + session_info.zeroth_delay_tranche_width)).saturating_sub(session_info.zeroth_delay_tranche_width)`.
    * `import_checked_assignment`
    * return the appropriate `VoteCheckResult` on the response channel.

#### `ApprovalVotingMessage::CheckAndImportApproval`

On receiving a `CheckAndImportApproval(indirect_approval_vote, response_channel)` message:
  * Fetch the `BlockEntry` from the indirect approval vote's `block_hash`. If none, return `VoteCheckResult::Bad`.
  * Fetch the `CandidateEntry` from the indirect approval vote's `candidate_index`. If the block did not trigger inclusion of enough candidates, return `VoteCheckResult::Bad`.
  * Construct a `SignedApprovalVote` using the candidate hash and check against the validator's approval key, based on the session info of the block. If invalid or no such validator, return `VoteCheckResult::Bad`.
  * Send `VoteCheckResult::Accepted`,
  * `import_checked_approval(BlockEntry, CandidateEntry, ValidatorIndex)`

#### `ApprovalVotingMessage::ApprovedAncestor`

On receiving an `ApprovedAncestor(Hash, BlockNumber, response_channel)`:
  * Iterate over the ancestry of the hash all the way back to block number given, starting from the provided block hash.
  * Keep track of an `all_approved_max: Option<Hash>`.
  * For each block hash encountered, load the `BlockEntry` associated. If any are not found, return `None` on the response channel and conclude.
  * If the block entry's `approval_bitfield` has all bits set to 1 and `all_approved_max == None`, set `all_approved_max = Some(current_hash)`.
  * If the block entry's `approval_bitfield` has any 0 bits, set `all_approved_max = None`.
  * After iterating all ancestry, return `all_approved_max`.

### Utility

#### `import_checked_assignment`
  * Load the candidate in question and access the `approval_entry` for the block hash the cert references.
  * Ensure the validator index is not part of the backing group for the candidate.
  * Ensure the validator index is not present in the approval entry already.
  * Create a tranche entry for the delay tranche in the approval entry and note the assignment within it.
  * Note the candidate index within the approval entry.

#### `import_checked_approval(BlockEntry, CandidateEntry, ValidatorIndex)`
  * Set the corresponding bit of the `approvals` bitfield in the `CandidateEntry` to `1`.
  * For each `ApprovalEntry` in the `CandidateEntry` (typically only 1), check whether the validator is assigned as a checker.
    * If so, set `n_tranches = tranches_to_approve(approval_entry)`.
    * If `check_approval(block_entry, approval_entry, n_tranches)` is true, set the corresponding bit in the `block_entry.approved_bitfield`.

#### `tranches_to_approve(approval_entry) -> tranches`
  * Determine the amount of tranches `n_tranches` our view of the protocol requires of this approval entry
    * First, take tranches until we have at least `session_info.needed_approvals`. Call the number of tranches taken `k`
    * Then, count no-shows in tranches `0..k`. For each no-show, we require another checker. Take new tranches until each no-show is covered, so now we've taken `l` tranches. e.g. if there are 2 no-shows, we might only need to take 1 additional tranche with >= 2 assignments. Or we might need to take 3 tranches, where one is empty and the other two have 1 assignment each.
    * Count no-shows in tranches `k..l` and for each of those, take tranches until all no-shows are covered. Repeat so on until either
      * We run out of tranches to take, having not received any assignments past a certain point. In this case we set `n_tranches` to a special value `ALL` which indicates that new assignments are needed.
      * All no-shows are covered. Set `n_tranches` to the number of tranches taken
    * return `n_tranches`

#### `check_approval(block_entry, approval_entry, n_tranches) -> bool`
  * If `n_tranches` is ALL, return false
  * Otherwise, if all validators in `n_tranches` have approved, return `true`. If any validator in these tranches has not yet approved but is not yet considered a no-show, return `false`.

#### `process_wakeup(relay_block, candidate_hash)`
  * Load the `BlockEntry` and `CandidateEntry` from disk. If either is not present, this may have lost a race with finality and can be ignored. Also load the `ApprovalEntry` for the block and candidate.
  * Set `n_tranches = tranches_to_approve(approval_entry)`
  * If `OurAssignment` has tranche `<= n_tranches`, the tranche is live according to our local clock (based against block slot), and we have not triggered the assignment already
    * Import to `ApprovalEntry`
    * Broadcast on network with an `ApprovalNetworkingMessage::DistributeAssignment`.
    * Kick off approval work with `launch_approval`
  * Schedule another wakeup based on `next_wakeup`

#### `next_wakeup(approval_entry, candidate_entry)`:
  * Return the earlier of our next no-show timeout or the tranche of our assignment, if not yet triggered
  * Our next no-show timeout is computed by finding the earliest-received assignment within `n_tranches` for which we have not received an approval and adding `to_ticks(session_info.no_show_slots)` to it.

#### `launch_approval(SessionIndex, CandidateDescriptor, ValidatorIndex, block_hash, candidate_index)`:
  * Extract the public key of the `ValidatorIndex` from the `SessionInfo` for the session.
  * Issue an `AvailabilityRecoveryMessage::RecoverAvailableData(candidate, session_index, response_sender)`
  * Load the historical validation code of the parachain by dispatching a `RuntimeApiRequest::HistoricalValidationCode(`descriptor.para_id`, `descriptor.relay_parent`)` against the state of `block_hash`.
  * Spawn a background task with a clone of `approval_vote_tx`
    * Wait for the available data
    * Issue a `CandidateValidationMessage::ValidateFromExhaustive` message
    * Wait for the result of validation
    * If valid, issue a message on `approval_vote_tx` detailing the request.

#### `issue_approval(request)`:
  * Fetch the block entry and candidate entry. Ignore if `None` - we've probably just lost a race with finality.
  * Construct a `SignedApprovalVote` with the validator index for the session.
  * Transform into an `IndirectSignedApprovalVote` using the `block_hash` and `candidate_index` from the request.
  * `import_checked_approval(block_entry, candidate_entry, validator_index)`
  * Dispatch an `ApprovalNetworkingMessage::DistributeApproval` message.
