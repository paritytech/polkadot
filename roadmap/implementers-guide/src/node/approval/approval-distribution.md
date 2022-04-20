# Approval Distribution

A subsystem for the distribution of assignments and approvals for approval checks on candidates over the network.

The [Approval Voting](approval-voting.md) subsystem is responsible for active participation in a protocol designed to select a sufficient number of validators to check each and every candidate which appears in the relay chain. Statements of participation in this checking process are divided into two kinds:
  - **Assignments** indicate that validators have been selected to do checking
  - **Approvals** indicate that validators have checked and found the candidate satisfactory.

The [Approval Voting](approval-voting.md) subsystem handles all the issuing and tallying of this protocol, but this subsystem is responsible for the disbursal of statements among the validator-set.

The inclusion pipeline of candidates concludes after availability, and only after inclusion do candidates actually get pushed into the approval checking pipeline. As such, this protocol deals with the candidates _made available by_ particular blocks, as opposed to the candidates which actually appear within those blocks, which are the candidates _backed by_ those blocks. Unless stated otherwise, whenever we reference a candidate partially by block hash, we are referring to the set of candidates _made available by_ those blocks.

We implement this protocol as a gossip protocol, and like other parachain-related gossip protocols our primary concerns are about ensuring fast message propagation while maintaining an upper bound on the number of messages any given node must store at any time.

Approval messages should always follow assignments, so we need to be able to discern two pieces of information based on our [View](../../types/network.md#universal-types):
  1. Is a particular assignment relevant under a given `View`?
  2. Is a particular approval relevant to any assignment in a set?

For our own local view, these two queries  must not yield false negatives. When applied to our peers' views, it is acceptable for them to yield false negatives. The reason for that is that our peers' views may be beyond ours, and we are not capable of fully evaluating them. Once we have caught up, we can check again for false negatives to continue distributing.

For assignments, what we need to be checking is whether we are aware of the (block, candidate) pair that the assignment references. For approvals, we need to be aware of an assignment by the same validator which references the candidate being approved.

However, awareness on its own of a (block, candidate) pair would imply that even ancient candidates all the way back to the genesis are relevant. We are actually not interested in anything before finality.

We gossip assignments along a grid topology produced by the [Gossip Support Subsystem](../utility/gossip-support.md) and also to a few random peers. The first time we accept an assignment or approval, regardless of the source, which originates from a validator peer in a shared dimension of the grid, we propagate the message to validator peers in the unshared dimension as well as a few random peers.

But, in case these mechanisms don't work on their own, we need to trade bandwidth for protocol liveness by introducing aggression.

Aggression has 3 levels:
    Aggression Level 0: The basic behaviors described above.
    Aggression Level 1: The originator of a message sends to all peers. Other peers follow the rules above.
    Aggression Level 2: All peers send all messages to all their row and column neighbors. This means that each validator will, on average, receive each message approximately 2*sqrt(n) times.

These aggression levels are chosen based on how long a block has taken to finalize: assignments and approvals related to the unfinalized block will be propagated with more aggression. In particular, it's only the earliest unfinalized blocks that aggression should be applied to, because descendants may be unfinalized only by virtue of being descendants.

## Protocol

Input:
  - `ApprovalDistributionMessage::NewBlocks`
  - `ApprovalDistributionMessage::DistributeAssignment`
  - `ApprovalDistributionMessage::DistributeApproval`
  - `ApprovalDistributionMessage::NetworkBridgeUpdate`
  - `OverseerSignal::BlockFinalized`

Output:
  - `ApprovalVotingMessage::CheckAndImportAssignment`
  - `ApprovalVotingMessage::CheckAndImportApproval`
  - `NetworkBridgeMessage::SendValidationMessage::ApprovalDistribution`

## Functionality

```rust
type BlockScopedCandidate = (Hash, CandidateHash);

enum PendingMessage {
  Assignment(IndirectAssignmentCert, CoreIndex),
  Approval(IndirectSignedApprovalVote),
}

/// The `State` struct is responsible for tracking the overall state of the subsystem.
///
/// It tracks metadata about our view of the unfinalized chain, which assignments and approvals we have seen, and our peers' views.
struct State {
  // These two fields are used in conjunction to construct a view over the unfinalized chain.
  blocks_by_number: BTreeMap<BlockNumber, Vec<Hash>>,
  blocks: HashMap<Hash, BlockEntry>,

  /// Our view updates to our peers can race with `NewBlocks` updates. We store messages received
  /// against the directly mentioned blocks in our view in this map until `NewBlocks` is received.
  ///
  /// As long as the parent is already in the `blocks` map and `NewBlocks` messages aren't delayed
  /// by more than a block length, this strategy will work well for mitigating the race. This is
  /// also a race that occurs typically on local networks.
  pending_known: HashMap<Hash, Vec<(PeerId, PendingMessage>)>>,

  // Peer view data is partially stored here, and partially inline within the `BlockEntry`s
  peer_views: HashMap<PeerId, View>,
}

enum MessageFingerprint {
  Assigment(Hash, u32, ValidatorIndex),
  Approval(Hash, u32, ValidatorIndex),
}

struct Knowledge {
  known_messages: HashSet<MessageFingerprint>,
}

struct PeerKnowledge {
  /// The knowledge we've sent to the peer.
  sent: Knowledge,
  /// The knowledge we've received from the peer.
  received: Knowledge,
}

/// Information about blocks in our current view as well as whether peers know of them.
struct BlockEntry {
  // Peers who we know are aware of this block and thus, the candidates within it. This maps to their knowledge of messages.
  known_by: HashMap<PeerId, PeerKnowledge>,
  // The number of the block.
  number: BlockNumber,
  // The parent hash of the block.
  parent_hash: Hash,
  // Our knowledge of messages.
  knowledge: Knowledge,
  // A votes entry for each candidate.
  candidates: IndexMap<CandidateHash, CandidateEntry>,
}

enum ApprovalState {
  Assigned(AssignmentCert),
  Approved(AssignmentCert, ApprovalSignature),
}

/// Information about candidates in the context of a particular block they are included in. In other words,
/// multiple `CandidateEntry`s may exist for the same candidate, if it is included by multiple blocks - this is likely the case
/// when there are forks.
struct CandidateEntry {
  approvals: HashMap<ValidatorIndex, ApprovalState>,
}
```

### Network updates

#### `NetworkBridgeEvent::PeerConnected`

Add a blank view to the `peer_views` state.

#### `NetworkBridgeEvent::PeerDisconnected`

Remove the view under the associated `PeerId` from `State::peer_views`.

Iterate over every `BlockEntry` and remove `PeerId` from it.

#### `NetworkBridgeEvent::OurViewChange`

Remove entries in `pending_known` for all hashes not present in the view.
Ensure a vector is present in `pending_known` for each hash in the view that does not have an entry in `blocks`.

#### `NetworkBridgeEvent::PeerViewChange`

Invoke `unify_with_peer(peer, view)` to catch them up to messages we have.

We also need to use the `view.finalized_number` to remove the `PeerId` from any blocks that it won't be wanting information about anymore. Note that we have to be on guard for peers doing crazy stuff like jumping their `finalized_number` forward 10 trillion blocks to try and get us stuck in a loop for ages.

One of the safeguards we can implement is to reject view updates from peers where the new `finalized_number` is less than the previous.

We augment that by defining `constrain(x)` to output the x bounded by the first and last numbers in `state.blocks_by_number`.

From there, we can loop backwards from `constrain(view.finalized_number)` until `constrain(last_view.finalized_number)` is reached, removing the `PeerId` from all `BlockEntry`s referenced at that height. We can break the loop early if we ever exit the bound supplied by the first block in `state.blocks_by_number`.

#### `NetworkBridgeEvent::PeerMessage`

If the block hash referenced by the message exists in `pending_known`, add it to the vector of pending messages and return.

If the message is of type `ApprovalDistributionV1Message::Assignment(assignment_cert, claimed_index)`, then call `import_and_circulate_assignment(MessageSource::Peer(sender), assignment_cert, claimed_index)`

If the message is of type `ApprovalDistributionV1Message::Approval(approval_vote)`, then call `import_and_circulate_approval(MessageSource::Peer(sender), approval_vote)`

### Subsystem Updates

#### `ApprovalDistributionMessage::NewBlocks`

Create `BlockEntry` and `CandidateEntries` for all blocks.

For all entries in `pending_known`:
  * If there is now an entry under `blocks` for the block hash, drain all messages and import with `import_and_circulate_assignment` and `import_and_circulate_approval`.

For all peers:
  * Compute `view_intersection` as the intersection of the peer's view blocks with the hashes of the new blocks.
  * Invoke `unify_with_peer(peer, view_intersection)`.

#### `ApprovalDistributionMessage::DistributeAsignment`

Call `import_and_circulate_assignment` with `MessageSource::Local`.

#### `ApprovalDistributionMessage::DistributeApproval`

Call `import_and_circulate_approval` with `MessageSource::Local`.

#### `OverseerSignal::BlockFinalized`

Prune all lists from `blocks_by_number` with number less than or equal to `finalized_number`. Prune all the `BlockEntry`s referenced by those lists.


### Utility

```rust
enum MessageSource {
  Peer(PeerId),
  Local,
}
```

#### `import_and_circulate_assignment(source: MessageSource, assignment: IndirectAssignmentCert, claimed_candidate_index: CandidateIndex)`

Imports an assignment cert referenced by block hash and candidate index. As a postcondition, if the cert is valid, it will have distributed the cert to all peers who have the block in their view, with the exclusion of the peer referenced by the `MessageSource`.

We maintain a few invariants:
  * we only send an assignment to a peer after we add its fingerprint to our knowledge
  * we add a fingerprint of an assignment to our knowledge only if it's valid and hasn't been added before

The algorithm is the following:

  * Load the `BlockEntry` using `assignment.block_hash`. If it does not exist, report the source if it is `MessageSource::Peer` and return.
  * Compute a fingerprint for the `assignment` using `claimed_candidate_index`.
  * If the source is `MessageSource::Peer(sender)`:
    * check if `peer` appears under `known_by` and whether the fingerprint is in the knowledge of the peer. If the peer does not know the block, report for providing data out-of-view and proceed. If the peer does know the block and the `sent` knowledge contains the fingerprint, report for providing replicate data and return, otherwise, insert into the `received` knowledge and return.
    * If the message fingerprint appears under the `BlockEntry`'s `Knowledge`, give the peer a small positive reputation boost,
    add the fingerprint to the peer's knowledge only if it knows about the block and return.
    Note that we must do this after checking for out-of-view and if the peers knows about the block to avoid being spammed.
    If we did this check earlier, a peer could provide data out-of-view repeatedly and be rewarded for it.
    * Dispatch `ApprovalVotingMessage::CheckAndImportAssignment(assignment)` and wait for the response.
    * If the result is `AssignmentCheckResult::Accepted`
      * If the vote was accepted but not duplicate, give the peer a positive reputation boost
      * add the fingerprint to both our and the peer's knowledge in the `BlockEntry`. Note that we only doing this after making sure we have the right fingerprint.
    * If the result is `AssignmentCheckResult::AcceptedDuplicate`, add the fingerprint to the peer's knowledge if it knows about the block and return.
    * If the result is `AssignmentCheckResult::TooFarInFuture`, mildly punish the peer and return.
    * If the result is `AssignmentCheckResult::Bad`, punish the peer and return.
  * If the source is `MessageSource::Local(CandidateIndex)`
    * check if the fingerprint appears under the `BlockEntry's` knowledge. If not, add it.
  * Load the candidate entry for the given candidate index. It should exist unless there is a logic error in the approval voting subsystem.
  * Set the approval state for the validator index to `ApprovalState::Assigned` unless the approval state is set already. This should not happen as long as the approval voting subsystem instructs us to ignore duplicate assignments.
  * Dispatch a `ApprovalDistributionV1Message::Assignment(assignment, candidate_index)` to all peers in the `BlockEntry`'s `known_by` set, excluding the peer in the `source`, if `source` has kind `MessageSource::Peer`. Add the fingerprint of the assignment to the knowledge of each peer.


#### `import_and_circulate_approval(source: MessageSource, approval: IndirectSignedApprovalVote)`

Imports an approval signature referenced by block hash and candidate index:

  * Load the `BlockEntry` using `approval.block_hash` and the candidate entry using `approval.candidate_entry`. If either does not exist, report the source if it is `MessageSource::Peer` and return.
  * Compute a fingerprint for the approval.
  * Compute a fingerprint for the corresponding assignment. If the `BlockEntry`'s knowledge does not contain that fingerprint, then report the source if it is `MessageSource::Peer` and return. All references to a fingerprint after this refer to the approval's, not the assignment's.
  * If the source is `MessageSource::Peer(sender)`:
    * check if `peer` appears under `known_by` and whether the fingerprint is in the knowledge of the peer. If the peer does not know the block, report for providing data out-of-view and proceed. If the peer does know the block and the `sent` knowledge contains the fingerprint, report for providing replicate data and return, otherwise, insert into the `received` knowledge and return.
    * If the message fingerprint appears under the `BlockEntry`'s `Knowledge`, give the peer a small positive reputation boost,
    add the fingerprint to the peer's knowledge only if it knows about the block and return.
    Note that we must do this after checking for out-of-view to avoid being spammed. If we did this check earlier, a peer could provide data out-of-view repeatedly and be rewarded for it.
    * Dispatch `ApprovalVotingMessage::CheckAndImportApproval(approval)` and wait for the response.
    * If the result is `VoteCheckResult::Accepted(())`:
      * Give the peer a positive reputation boost and add the fingerprint to both our and the peer's knowledge.
    * If the result is `VoteCheckResult::Bad`:
      * Report the peer and return.
  * Load the candidate entry for the given candidate index. It should exist unless there is a logic error in the approval voting subsystem.
  * Set the approval state for the validator index to `ApprovalState::Approved`. It should already be in the `Assigned` state as our `BlockEntry` knowledge contains a fingerprint for the assignment.
  * Dispatch a `ApprovalDistributionV1Message::Approval(approval)` to all peers in the `BlockEntry`'s `known_by` set, excluding the peer in the `source`, if `source` has kind `MessageSource::Peer`. Add the fingerprint of the assignment to the knowledge of each peer. Note that this obeys the politeness conditions:
    * We guarantee elsewhere that all peers within `known_by` are aware of all assignments relative to the block.
    * We've checked that this specific approval has a corresponding assignment within the `BlockEntry`.
    * Thus, all peers are aware of the assignment or have a message to them in-flight which will make them so.


#### `unify_with_peer(peer: PeerId, view)`:

1. Initialize a set `missing_knowledge = {}`

For each block in the view:
  2. Load the `BlockEntry` for the block. If the block is unknown, or the number is less than or equal to the view's finalized number go to step 6.
  3. Inspect the `known_by` set of the `BlockEntry`. If the peer already knows all assignments/approvals, go to step 6.
  4. Add the peer to `known_by` and add the hash and missing knowledge of the block to `missing_knowledge`.
  5. Return to step 2 with the ancestor of the block.

6. For each block in `missing_knowledge`, send all assignments and approvals for all candidates in those blocks to the peer.
