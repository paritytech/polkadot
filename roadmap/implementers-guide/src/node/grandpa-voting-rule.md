# GRANDPA Voting Rule

Specifics on the motivation and types of constraints we apply to the GRANDPA voting logic as well as the definitions of **viable** and **finalizable** blocks can be found in the [Chain Selection Protocol](../protocol-chain-selection.md) section.
The subsystem which provides us with viable leaves is the [Chain Selection Subsystem](utility/chain-selection.md). 

GRANDPA's regular voting rule is for each validator to select the longest chain they are aware of. GRANDPA proceeds in rounds, collecting information from all online validators and determines the blocks that a supermajority of validators all have in common with each other.

The low-level GRANDPA logic will provide us with a **required block**. We can find the best leaf containing that block in its chain with the [`ChainSelectionMessage::BestLeafContaining`](../types/overseer-protocol.md#chain-selection-message). If the result is `None`, then we will simply cast a vote on the required block.

The **viable** leaves provided from the chain selection subsystem are not necessarily **finalizable**, so we need to perform further work to discover the finalizable ancestor of the block. The first constraint is to avoid voting on any unapproved block. The highest approved ancestor of a given block can be determined by querying the Approval Voting subsystem via the [`ApprovalVotingMessage::ApprovedAncestor`](../types/overseer-protocol.md#approval-voting) message. If the response is `Some`, we continue and apply the second constraint. The second constraint is to avoid voting on any block containing a candidate undergoing an active dispute. The list of block hashes and candidates returned from `ApprovedAncestor` should be reversed, and passed to the [`DisputeCoordinatorMessage::DetermineUndisputedChain`](../types/overseer-protocol.md#dispute-coordinator-message) to determine the **finalizable** block which will be our eventual vote.
