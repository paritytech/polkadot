# Approval Voting

Reading the [section on the approval protocol](../../protocol-approval.md) will likely be necessary to understand the aims of this subsystem.

Approval votes are split into two parts: Assignments and Approvals. Validators first broadcast their assignment to indicate intent to check a candidate. Upon successfully checking, they broadcast an approval vote. If a validator doesn't broadcast their approval vote shortly after issuing an assignment, this is an indication that they are being prevented from recovering or validating the block data and that more validators should self-select to check the candidate. This is known as a "no-show".

The core of this subsystem is a Tick-based timer loop, where Ticks are 500ms. We also reason about time in terms of DelayTranches, which measure the number of ticks elapsed since a block was produced. We track metadata for all un-finalized but included candidates. We compute our local assignments to check each candidate, as well as which DelayTranche those assignments may be minimally triggered at. As the same candidate may appear in more than one block, we must produce our potential assignments for each (Block, Candidate) pair. The timing loop is based on waiting for assignments to become no-shows or waiting to broadcast and begin our own assignment to check.

Another main component of this subsystem is the logic for determining when a (Block, Candidate) pair has been approved and when to broadcast and trigger our own assignment. Once a (Block, Candidate) pair has been approved, we mark a corresponding bit in the BlockEntry that indicates the candidate has been approved under the block. When we trigger our own assignment, we broadcast it via Approval Distribution, begin fetching the data from Availability Recovery, and then pass it through to the Candidate Validation. Once these steps are successful, we issue our approval vote. If any of these steps fail, we don't issue any vote and will "no-show" from the perspective of other validators. In the future we will initiate disputes as well.

Where this all fits into Polkadot is via block finality. Our goal is to not finalize any block containing a candidate that is not approved. We provide a hook for a custom GRANDPA voting rule - GRANDPA makes requests of the form (target, minimum) consisting of a target block (i.e. longest chain) that it would like to finalize, and a minimum block which, due to the rules of GRANDPA, must be voted on. The minimum is typically the last finalized block, but may be beyond it, in the case of having a last-round-estimate beyond the last finalized. Thus, our goal is to inform GRANDPA of some block between target and minimum which we believe can be finalized safely. We do this by iterating backwards from the target to the minimum and finding the longest continuous chain from minimum where all candidates included by those blocks have been approved.

## Protocol

Input:
  - `ApprovalVotingMessage::CheckAndImportAssignment`
  - `ApprovalVotingMessage::CheckAndImportApproval`
  - `ApprovalVotingMessage::ApprovedAncestor`

Output:
  - `ApprovalDistributionMessage::DistributeAssignment`
  - `ApprovalDistributionMessage::DistributeApproval`
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
    slot: Slot,
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
    children: Vec<Hash>,
}

// slot_duration * 2 + DelayTranche gives the number of delay tranches since the
// unix epoch.
type Tick = u64;

struct StoredBlockRange(BlockNumber, BlockNumber);
```

In the schema, we map

```
"StoredBlocks" => StoredBlockRange
BlockNumber => Vec<BlockHash>
BlockHash => BlockEntry
CandidateHash => CandidateEntry
```

## Logic

```rust
const APPROVAL_SESSIONS: SessionIndex = 6;
```

In-memory state:

```rust
struct ApprovalVoteRequest {
  validator_index: ValidatorIndex,
  block_hash: Hash,
  candidate_index: CandidateIndex,
}

// Requests that background work (approval voting tasks) may need to make of the main subsystem
// task.
enum BackgroundRequest {
  ApprovalVote(ApprovalVoteRequest),
  // .. others, unspecified as per implementation.
}

// This is the general state of the subsystem. The actual implementation may split this
// into further pieces.
struct State {
    earliest_session: SessionIndex,
    session_info: Vec<SessionInfo>,
    babe_epoch: Option<BabeEpoch>, // information about a cached BABE epoch.
    keystore: KeyStorePtr,

    // A scheduler which keeps at most one wakeup per hash, candidate hash pair and
    // maps such pairs to `Tick`s.
    wakeups: Wakeups, 

    // These are connected to each other.
    background_tx: mpsc::Sender<BackgroundRequest>,
    background_rx: mpsc::Receiver<BackgroundRequest>,
}
```

This guide section makes no explicit references to writes to or reads from disk. Instead, it handles them implicitly, with the understanding that updates to block, candidate, and approval entries are persisted to disk.

[`SessionInfo`](../../runtime/session_info.md)

On start-up, we clear everything currently stored by the database. This is done by loading the `StoredBlockRange`, iterating through each block number, iterating through each block hash, and iterating through each candidate referenced by each block. Although this is `O(o*n*p)`, we don't expect to have more than a few unfinalized blocks at any time and in extreme cases, a few thousand. The clearing operation should be relatively fast as a result.

Main loop:
  * Each iteration, select over all of
    * The next `Tick` in `wakeups`: trigger `wakeup_process` for each `(Hash, Hash)` pair scheduled under the `Tick` and then remove all entries under the `Tick`.
    * The next message from the overseer: handle the message as described in the [Incoming Messages section](#incoming-messages)
    * The next approval vote request from `background_rx`
      * If this is an `ApprovalVoteRequest`, [Issue an approval vote](#issue-approval-vote).

### Incoming Messages

#### `OverseerSignal::BlockFinalized`

On receiving an `OverseerSignal::BlockFinalized(h)`, we fetch the block number `b` of that block from the ChainApi subsystem. We update our `StoredBlockRange` to begin at `b+1`. Additionally, we remove all block entries and candidates referenced by them up to and including `b`. Lastly, we prune out all descendents of `h` transitively: when we remove a `BlockEntry` with number `b` that is not equal to `h`, we recursively delete all the `BlockEntry`s referenced as children. We remove the `block_assignments` entry for the block hash and if `block_assignments` is now empty, remove the `CandidateEntry`. We also update each of the `BlockNumber -> Vec<Hash>` keys in the database to reflect the blocks at that height, clearing if empty.


#### `OverseerSignal::ActiveLeavesUpdate`

On receiving an `OverseerSignal::ActiveLeavesUpdate(update)`:
  * We determine the set of new blocks that were not in our previous view. This is done by querying the ancestry of all new items in the view and contrasting against the stored `BlockNumber`s. Typically, there will be only one new block. We fetch the headers and information on these blocks from the ChainApi subsystem.
  * We update the `StoredBlockRange` and the `BlockNumber` maps.
  * We use the RuntimeApiSubsystem to determine information about these blocks. It is generally safe to assume that runtime state is available for recent, unfinalized blocks. In the case that it isn't, it means that we are catching up to the head of the chain and needn't worry about assignments to those blocks anyway, as the security assumption of the protocol tolerates nodes being temporarily offline or out-of-date.
    * We fetch the set of candidates included by each block by dispatching a `RuntimeApiRequest::CandidateEvents` and checking the `CandidateIncluded` events.
    * We fetch the session of the block by dispatching a `session_index_for_child` request with the parent-hash of the block.
    * If the `session index - APPROVAL_SESSIONS > state.earliest_session`, then bump `state.earliest_sessions` to that amount and prune earlier sessions.
    * If the session isn't in our `state.session_info`, load the session info for it and for all sessions since the earliest-session, including the earliest-session, if that is missing. And it can be, just after pruning, if we've done a big jump forward, as is the case when we've just finished chain synchronization.
    * If any of the runtime API calls fail, we just warn and skip the block.
  * We use the RuntimeApiSubsystem to determine the set of candidates included in these blocks and use BABE logic to determine the slot number and VRF of the blocks.
  * We also note how late we appear to have received the block. We create a `BlockEntry` for each block and a `CandidateEntry` for each candidate obtained from `CandidateIncluded` events after making a `RuntimeApiRequest::CandidateEvents` request.
  * Ensure that the `CandidateEntry` contains a `block_assignments` entry for the block, with the correct backing group set.
  * If a validator in this session, compute and assign `our_assignment` for the `block_assignments`
    * Only if not a member of the backing group.
    * Run `RelayVRFModulo` and `RelayVRFDelay` according to the [the approvals protocol section](../../protocol-approval.md#assignment-criteria). Ensure that the assigned core derived from the output is covered by the auxiliary signature aggregated in the `VRFPRoof`.
  * [Handle Wakeup](#handle-wakeup) for each new candidate in each new block - this will automatically broadcast a 0-tranche assignment, kick off approval work, and schedule the next delay.
  * Dispatch an `ApprovalDistributionMessage::NewBlocks` with the meta information filled out for each new block.

#### `ApprovalVotingMessage::CheckAndImportAssignment`

On receiving a `ApprovalVotingMessage::CheckAndImportAssignment` message, we check the assignment cert against the block entry. The cert itself contains information necessary to determine the candidate that is being assigned-to. In detail:
  * Load the `BlockEntry` for the relay-parent referenced by the message. If there is none, return `AssignmentCheckResult::Bad`.
  * Fetch the `SessionInfo` for the session of the block
  * Determine the assignment key of the validator based on that.
  * Determine the claimed core index by looking up the candidate with given index in `block_entry.candidates`. Return `AssignmentCheckResult::Bad` if missing.
  * Check the assignment cert
    * If the cert kind is `RelayVRFModulo`, then the certificate is valid as long as `sample < session_info.relay_vrf_samples` and the VRF is valid for the validator's key with the input `block_entry.relay_vrf_story ++ sample.encode()` as described with [the approvals protocol section](../../protocol-approval.md#assignment-criteria). We set `core_index = vrf.make_bytes().to_u32() % session_info.n_cores`. If the `BlockEntry` causes inclusion of a candidate at `core_index`, then this is a valid assignment for the candidate at `core_index` and has delay tranche 0. Otherwise, it can be ignored.
    * If the cert kind is `RelayVRFDelay`, then we check if the VRF is valid for the validator's key with the input `block_entry.relay_vrf_story ++ cert.core_index.encode()` as described in [the approvals protocol section](../../protocol-approval.md#assignment-criteria). The cert can be ignored if the block did not cause inclusion of a candidate on that core index. Otherwise, this is a valid assignment for the included candidate. The delay tranche for the assignment is determined by reducing `(vrf.make_bytes().to_u64() % (session_info.n_delay_tranches + session_info.zeroth_delay_tranche_width)).saturating_sub(session_info.zeroth_delay_tranche_width)`.
    * We also check that the core index derived by the output is covered by the `VRFProof` by means of an auxiliary signature.
    * If the delay tranche is too far in the future, return `AssignmentCheckResult::TooFarInFuture`.
  * Import the assignment.
    * Load the candidate in question and access the `approval_entry` for the block hash the cert references.
    * Ignore if we already observe the validator as having been assigned.
    * Ensure the validator index is not part of the backing group for the candidate.
    * Ensure the validator index is not present in the approval entry already.
    * Create a tranche entry for the delay tranche in the approval entry and note the assignment within it.
    * Note the candidate index within the approval entry.
  * [Check for full approval of the candidate entry](#check-full-approval) of the candidate_entry, filtering by this specific approval entry.
  * return the appropriate `AssignmentCheckResult` on the response channel.

#### `ApprovalVotingMessage::CheckAndImportApproval`

On receiving a `CheckAndImportApproval(indirect_approval_vote, response_channel)` message:
  * Fetch the `BlockEntry` from the indirect approval vote's `block_hash`. If none, return `ApprovalCheckResult::Bad`.
  * Fetch the `CandidateEntry` from the indirect approval vote's `candidate_index`. If the block did not trigger inclusion of enough candidates, return `ApprovalCheckResult::Bad`.
  * Construct a `SignedApprovalVote` using the candidate hash and check against the validator's approval key, based on the session info of the block. If invalid or no such validator, return `ApprovalCheckResult::Bad`.
  * Send `ApprovalCheckResult::Accepted`
  * [Import the checked approval vote](#import-checked-approval)

#### `ApprovalVotingMessage::ApprovedAncestor`

On receiving an `ApprovedAncestor(Hash, BlockNumber, response_channel)`:
  * Iterate over the ancestry of the hash all the way back to block number given, starting from the provided block hash.
  * Keep track of an `all_approved_max: Option<(Hash, BlockNumber)>`.
  * For each block hash encountered, load the `BlockEntry` associated. If any are not found, return `None` on the response channel and conclude.
  * If the block entry's `approval_bitfield` has all bits set to 1 and `all_approved_max == None`, set `all_approved_max = Some((current_hash, current_number))`.
  * If the block entry's `approval_bitfield` has any 0 bits, set `all_approved_max = None`.
  * After iterating all ancestry, return `all_approved_max`.

### Updates and Auxiliary Logic

#### Import Checked Approval
  * Import an approval vote which we can assume to have passed signature checks.
  * Requires `(BlockEntry, CandidateEntry, ValidatorIndex)`
  * Set the corresponding bit of the `approvals` bitfield in the `CandidateEntry` to `1`. If already `1`, return.
  * [Check full approval of the candidate](#check-full-approval)

#### Check Full Approval
  * Checks the approval state of the candidate under every block it is included by, and updates the block entries accordingly.
  * Requires `(CandidateEntry, filter)`, where filter is used to limit which approval entries are inspected.
  * Checks every `ApprovalEntry` that is not yet `approved` for whether it is now approved.
    * For each `ApprovalEntry` in the `CandidateEntry` that is not `approved` and passes the `filter`
    * Load the block entry for the `ApprovalEntry`.
    * If so, [determine the tranches to inspect](#determine-required-tranches) of the candidate,
    * If [the candidate is approved under the block](#check-approval), set the corresponding bit in the `block_entry.approved_bitfield`.
    * Otherwise, [schedule a wakeup of the candidate](#schedule-wakeup)

#### Handling Wakeup
  * Handle a previously-scheduled wakeup of a candidate under a specific block.
  * Requires `(relay_block, candidate_hash)`
  * Load the `BlockEntry` and `CandidateEntry` from disk. If either is not present, this may have lost a race with finality and can be ignored. Also load the `ApprovalEntry` for the block and candidate.
  * [determine the `RequiredTranches` of the candidate](#determine-required-tranches).
  * Determine if we should trigger our assignment.
    * If we've already triggered or `OurAssignment` is `None`, we do not trigger.
    * If we have  `RequiredTranches::All`, then we trigger if the candidate is [not approved](#check-approval). We have no next wakeup as we assume that other validators are doing the same and we will be implicitly woken up by handling new votes.
    * If we have `RequiredTranches::Pending { considered, next_no_show, uncovered, maximum_broadcast, clock_drift }`, then we trigger if our assignment's tranche is less than or equal to `maximum_broadcast` and the current tick, with `clock_drift` applied, is at least the tick of our tranche. 
    * If we have `RequiredTranches::Exact { .. }` then we do not trigger, because this value indicates that no new assignments are needed at the moment.
  * If we should trigger our assignment
    * Import the assignment to the `ApprovalEntry`
    * Broadcast on network with an `ApprovalDistributionMessage::DistributeAssignment`.
    * [Launch approval work](#launch-approval-work) for the candidate.
  * [Schedule a new wakeup](#schedule-wakeup) of the candidate.

#### Schedule Wakeup
  * Requires `(approval_entry, candidate_entry)` which effectively denotes a `(Block Hash, Candidate Hash)` pair - the candidate, along with the block it appears in.
  * Also requires `RequiredTranches`
  * If the `approval_entry` is approved, this doesn't need to be woken up again.
  * If `RequiredTranches::All` - no wakeup. We assume other incoming votes will trigger wakeup and potentially re-schedule.
  * If `RequiredTranches::Pending { considered, next_no_show, uncovered, maximum_broadcast, clock_drift }` - schedule at the lesser of the next no-show tick, or the tick, offset positively by `clock_drift` of the next non-empty tranche we are aware of after `considered`, including any tranche containing our own unbroadcast assignment. This can lead to no wakeup in the case that we have already broadcast our assignment and there are no pending no-shows; that is, we have approval votes for every assignment we've received that is not already a no-show. In this case, we will be re-triggered by other validators broadcasting their assignments.
  * If `RequiredTranches::Exact { next_no_show, .. } - set a wakeup for the next no-show tick.

#### Launch Approval Work
  * Requires `(SessionIndex, SessionInfo, CandidateReceipt, ValidatorIndex, backing_group, block_hash, candidate_index)`
  * Extract the public key of the `ValidatorIndex` from the `SessionInfo` for the session.
  * Issue an `AvailabilityRecoveryMessage::RecoverAvailableData(candidate, session_index, Some(backing_group), response_sender)`
  * Load the historical validation code of the parachain by dispatching a `RuntimeApiRequest::HistoricalValidationCode(`descriptor.para_id`, `descriptor.relay_parent`)` against the state of `block_hash`.
  * Spawn a background task with a clone of `background_tx`
    * Wait for the available data
    * Issue a `CandidateValidationMessage::ValidateFromExhaustive` message
    * Wait for the result of validation
    * If valid, issue a message on `background_tx` detailing the request.

#### Issue Approval Vote
  * Fetch the block entry and candidate entry. Ignore if `None` - we've probably just lost a race with finality.
  * Construct a `SignedApprovalVote` with the validator index for the session.
  * [Import the checked approval vote](#import-checked-approval). It is "checked" as we've just issued the signature.
  * Construct a `IndirectSignedApprovalVote` using the information about the vote.
  * Dispatch `ApprovalDistributionMessage::DistributeApproval`.

### Determining Approval of Candidate

#### Determine Required Tranches

This logic is for inspecting an approval entry that tracks the assignments received, along with information on which assignments have corresponding approval votes. Inspection also involves the current time and expected requirements and is used to help the higher-level code determine the following:
  * Whether to broadcast the local assignment
  * Whether to check that the candidate entry has been completely approved.
  * If the candidate is waiting on approval, when to schedule the next wakeup of the `(candidate, block)` pair at a point where the state machine could be advanced.

These routines are pure functions which only depend on the environmental state. The expectation is that this determination is re-run every time we attempt to update an approval entry: either when we trigger a wakeup to advance the state machine based on a no-show or our own broadcast, or when we receive further assignments or approvals from the network.

Thus it may be that at some point in time, we consider that tranches 0..X is required to be considered, but as we receive more information, we might require fewer tranches. Or votes that we perceived to be missing and require replacement are filled in and change our view.

Requires `(approval_entry, approvals_received, tranche_now, block_tick, no_show_duration, needed_approvals)`

```rust
enum RequiredTranches {
  // All validators appear to be required, based on tranches already taken and remaining no-shows.
  All,
  // More tranches required - We're awaiting more assignments.
  Pending {
    /// The highest considered delay tranche when counting assignments.
    considered: DelayTranche,
    /// The tick at which the next no-show, of the assignments counted, would occur.
    next_no_show: Option<Tick>,
    /// The highest tranche to consider when looking to broadcast own assignment.
    /// This should be considered along with the clock drift to avoid broadcasting
    /// assignments that are before the local time.
    maximum_broadcast: DelayTranche,
    /// The clock drift, in ticks, to apply to the local clock when determining whether
    /// to broadcast an assignment or when to schedule a wakeup. The local clock should be treated
    /// as though it is `clock_drift` ticks earlier.
    clock_drift: Tick,
  },
  // An exact number of required tranches and a number of no-shows. This indicates that the amount of `needed_approvals` are assigned and additionally all no-shows are covered.
  Exact {
    /// The tranche to inspect up to.
    needed: DelayTranche,
    /// The amount of missing votes that should be tolerated.
    tolerated_missing: usize,
    /// When the next no-show would be, if any. This is used to schedule the next wakeup in the
    /// event that there are some assignments that don't have corresponding approval votes. If this
    /// is `None`, all assignments have approvals.
    next_no_show: Option<Tick>,
  }
}
```

**Clock-drift and Tranche-taking**

Our vote-counting procedure depends heavily on how we interpret time based on the presence of no-shows - assignments which have no corresponding approval after some time.

We have this is because of how we handle no-shows: we keep track of the depth of no-shows we are covering. 

As an example: there may be initial no-shows in tranche 0. It'll take `no_show_duration` ticks before those are considered no-shows. Then, we don't want to immediately take `no_show_duration` more tranches. Instead, we want to take one tranche for each uncovered no-show. However, as we take those tranches, there may be further no-shows. Since these depth-1 no-shows should have only been triggered after the depth-0 no-shows were already known to be no-shows, we need to discount the local clock by `no_show_duration` to  see whether these should be considered no-shows or not. There may be malicious parties who broadcast their assignment earlier than they were meant to, who shouldn't be counted as instant no-shows. We continue onwards to cover all depth-1 no-shows which may lead to depth-2 no-shows and so on.

Likewise, when considering how many tranches to take, the no-show depth should be used to apply a depth-discount or clock drift to the `tranche_now`.

**Procedure**

  * Start with `depth = 0`.
  * Set a clock drift of `depth * no_show_duration`
  * Take tranches up to `tranche_now - clock_drift` until all needed assignments are met.
  * Keep track of the `next_no_show` according to the clock drift, as we go.
  * If running out of tranches before then, return `Pending { considered, next_no_show, maximum_broadcast, clock_drift }`
  * If there are no no-shows, return `Exact { needed, tolerated_missing, next_no_show }`
  * `maximum_broadcast` is either `DelayTranche::max_value()` at tranche 0 or otherwise by the last considered tranche + the number of uncovered no-shows at this point.
  * If there are no-shows, return to the beginning, incrementing `depth` and attempting to cover the number of no-shows. Each no-show must be covered by a non-empty tranche, which are tranches that have at least one assignment. Each non-empty tranche covers exactly one no-show.
  * If at any point, it seems that all validators are required, do an early return with `RequiredTranches::All` which indicates that everyone should broadcast.

#### Check Approval
  * Check whether a candidate is approved under a particular block.
  * Requires `(block_entry, candidate_entry, approval_entry, n_tranches)`
  * If `n_tranches` is `RequiredTranches::Pending`, return false
  * If `n_tranches` is `RequiredTranches::All`,  then we return `3 * n_approvals > 2 * n_validators`.
  * If `n_tranches` is `RequiredTranches::Exact { tranche, tolerated_missing, .. }`, then we return whether all assigned validators up to `tranche` less `tolerated_missing` have approved. e.g. if we had 5 tranches and 1 tolerated missing, we would accept only if all but 1 of assigned validators in tranches 0..=5 have approved. In that example, we also accept all validators in tranches 0..=5 having approved, but that would indicate that the `RequiredTranches` value was incorrectly constructed, so it is not realistic. `tolerated_missing` actually represents covered no-shows. If there are more missing approvals than there are tolerated missing, that indicates that there are some assignments which are not yet no-shows, but may become no-shows, and we should wait for the validators to either approve or become no-shows.

### Time

#### Current Tranche
  * Given the slot number of a block, and the current time, this informs about the current tranche.
  * Convert `time.saturating_sub(slot_number.to_time())` to a delay tranches value
