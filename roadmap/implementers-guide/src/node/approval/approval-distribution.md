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

These two queries need not be perfect, but they must never yield false positives. For our own local view, they must not yield false negatives. When applied to our peers' views, it is acceptable for them to yield false negatives. The reason for that is that our peers' views may be beyond ours, and we are not capable of fully evaluating them. Once we have caught up, we can check again for false negatives to continue distributing.

For assignments, what we need to be checking is whether we are aware of the (block, candidate) pair that the assignment references. For approvals, we need to be aware of an assignment by the same validator which references the candidate being approved.

However, awareness on its own of a (block, candidate) pair would imply that even ancient candidates all the way back to the genesis are relevant. We are actually not interested in anything before finality. 


## Protocol

## Functionality

```rust
type BlockScopedCandidate = (Hash, CandidateHash);

/// The `State` struct is responsible for tracking the overall state of the subsystem.
///
/// It tracks metadata about our view of the chain, which assignments and approvals we have seen, and our peers' views.
struct State {
  blocks_by_number: BTreeMap<BlockNumber, Vec<Hash>>,
  blocks: HashMap<Hash, BlockEntry>,
  peer_views: HashMap<PeerId, View>,
  finalized_number: BlockNumber,
}

/// Information about blocks in our current view as well as whether peers know of them.
struct BlockEntry {
  // Peers who we know are aware of this block and thus, the candidates within it.
  known_by: HashSet<PeerId>,
  // The number of the block.
  number: BlockNumber,
  // The parent hash of the block.
  parent_hash: Hash,
  // A votes entry for each candidate.
  candidates: IndexMap<CandidateHash, CandidateEntry>,
}

enum ApprovalState {
  Assigned(AssignmentCert),
  Approved(AssignmentCert, ApprovalSignature),
}

/// Information about candidates in the context of a particular block they are included in. In other words,
/// multiple `CandidateEntry`s may exist for the same candidate, if it is included by multiple blocks - this is likely the case /// when there are forks.
struct CandidateEntry {
  approvals: HashMap<ValidatorIndex, ApprovalState>,
}
```

### Network updates

#### `NetworkBridgeEvent::PeerConnected`

Add a blank view to the `peer_views` state.

#### `NetworkBridgeEvent::PeerDisconnected`

Remove the view under the associated `PeerId` from `State::peer_views`.

TODO: pruning? hard to see how to do it without just iterating over each `BlockEntry` but that might be rel. fast.

#### `NetworkBridgeEvent::PeerViewChange`

For each block in the view:
  1. Initialize `fresh_blocks = {}`
  2. Load the `BlockEntry` for the block. If the block is unknown, or the number is less than the view's finalized number, go to step 6.
  3. Inspect the `known_by` set. If the peer is already present, go to step 6.
  4. Add the peer to `known_by` and add the hash of the block to `fresh_blocks`.
  5. Return to step 2 with the ancestor of the block.
  6. For each block in `fresh_blocks`, send all assignments and approvals for all candidates in those blocks to the peer.

We also need to use the `view.finalized_number` to remove the `PeerId` from any blocks that it won't be wanting information about anymore. Note that we have to be on guard for peers doing crazy stuff like jumping their 'finalized_number` forward 10 trillion blocks to try and get us stuck in a loop for ages.

One of the safeguards we can implement is to reject view updates from peers where the new `finalized_number` is less than the previous. 

We augment that by defining `constrain(x)` to output the x bounded by the first and last numbers in `state.blocks_by_number`.

From there, we can loop backwards from `constrain(view.finalized_number)` until `constrain(last_view.finalized_number)` is reached, removing the `PeerId` from all `BlockEntry`s referenced at that height. We can break the loop early if we ever exit the bound supplied by the first block in `state.blocks_by_number`. 

#### `NetworkBridgeEvent::OurViewChange`

Prune all lists from `blocks_by_number` with number less than or equal to `view.finalized_number`. Prune all the `BlockEntry`s referenced by those lists.

#### `NetworkBridgeEvent::PeerMessage`

TODO: provide step-by-step for each of these checks.

If the message is an assignment,
  * Check if we are already aware of this assignment. If so, ignore, but note that the peer is aware of the assignment also. If the peer was already aware of the assignment, give a minor reputation punishment. Otherwise, give a small reputation bump. Note that if we don't accept the assignment, we will not be aware of it in our state and will proceed to the next step.
  * Check if we accept this assignment. If not, ignore & report.
  * Issue an `ApprovalVotingMessage::CheckAndImportAssignment`. If the result is `VoteCheckResult::Bad`, ignore & report. If the result is `VoteCheckResult::Ignore`, just ignore. If the result is `VoteCheckResult::Accepted`, store the assignment and note that the peer is aware of the assignment.
  * Distribute the assignment to all peers who will accept it.
  * Distribute any relevant approvals to all peers who will accept them. We could not send peers approval messages before they're aware of an assignment.

If the message is an approval,
  * Check if the peer is already aware of the approval. If so, ignore & report.
  * Check if we are already aware of this approval. If so, ignore, but note that the peer is aware of the approval also. Give a small reputation bump. Note that if we don't accept the approval, we will not be aware of it in our state and will proceed to the next step.
  * Check if we accept this approval. If not, ignore & report.
  * Issue an `ApprovalVotingMessage::CheckAndImportApproval`. If the result is `VoteCheckResult::Bad`, ignore & report. If the result is `VoteCheckResult::Ignore`, just ignore. If the result is `VoteCheckResult::Accepted`, store the approval and note that the peer is aware of the approval.
  * Distribute the approval to all peers who will accept it.

### Subsystem Updates

#### `AvailabilityDistributionMessage::NewBlocks`

TODO: create `BlockEntry` and `CandidateEntries` for all blocks.
TODO: check for new commonality with peers. Broadcast new information to peers we now understand better.
