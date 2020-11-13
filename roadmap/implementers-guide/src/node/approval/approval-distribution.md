# Approval Distribution

A subsystem for the distribution of assignments and approvals for approval checks on candidates over the network.

The [Approval Voting](approval-voting.md) subsystem is responsible for active participation in a protocol designed to select a sufficient number of validators to check each and every candidate which appears in the relay chain. Statements of participation in this checking process are divided into two kinds:
  - **Assignments** indicate that validators have been selected to do checking
  - **Approvals** indicate that validators have checked and found the candidate satisfactory.

The [Approval Voting](approval-voting.md) subsystem handles all the issuing and tallying of this protocol, but this subsystem is responsible for the disbursal of statements among the validator-set.

We implement this protocol as a gossip protocol, and like other parachain-related gossip protocols our primary concerns are about ensuring fast message propagation while maintaining an upper bound on the number of messages any given node must store at any time.

Approval messages should always follow assignments, so we need to be able to discern two pieces of information based on our [View](../../types/network.md#universal-types):
  1. Is a particular assignment relevant under a given `View`?
  2. Is a particular approval relevant to any assignment in a set?

These two queries need not be perfect, but they must never yield false positives. For our own local view, they should not yield false negatives. When applied to our peers' views, it is acceptable for them to yield false negatives. The reason for that is that our peers' views may be beyond ours, and we are not capable of fully evaluating them. Once we have caught up, we can check again for false negatives to continue distributing.

For assignments, what we need to be checking is whether we are aware of the (block, candidate) pair that the assignment references. For approvals, we need to be aware of an assignment by the same validator which references the candidate being approved.

However, awareness on its own of a (block, candidate) pair would imply that even ancient candidates all the way back to the genesis are relevant. We are actually not interested in anything before finality. did


## Protocol

## Functionality

### Network updates

#### `NetworkBridgeEvent::PeerConnected`

TODO: add a peer entry to state

#### `NetworkBridgeEvent::PeerDisconnected`

TODO: remove peer entry from state

#### `NetworkBridgeEvent::PeerViewChange`

TODO: find all blocks in common with the peer.

#### `NetworkBridgeEvent::OurViewChange`

TODO: find all blocks in common with all peers.

#### `NetworkBridgeEvent::PeerMessage`

TODO: provide step-by-step for each of these checks.

If the message is an assignment,
  * Check if the peer is already aware of the assignment. If so, ignore & report.
  * Check if we are already aware of this assignment. If so, ignore, but note that the peer is aware of the assignment also. Give a small reputation bump. Note that if we don't accept the assignment, we will not be aware of it in our state and will proceed to the next step.
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