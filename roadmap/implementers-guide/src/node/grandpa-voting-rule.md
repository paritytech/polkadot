# GRANDPA Voting Rule

[GRANDPA](https://w3f-research.readthedocs.io/en/latest/polkadot/finality.html) is the finality engine of Polkadot.

One broad goal of finality, which applies across many different blockchains, is that there should exist only one finalized block at each height in the finalized chain. Before a block at a given height is finalized, it may compete with other forks.

GRANDPA's regular voting rule is for each validator to select the longest chain they are aware of. GRANDPA proceeds in rounds, collecting information from all online validators and determines the blocks that a supermajority of validators all have in common with each other.

For parachains, we extend the security guarantee of finality to be such that no invalid parachain candidate may be included in a finalized block. Candidates may be included in some fork of the relay chain with only a few backing votes behind them. After that point, we run the [Approvals Protocol](../protocol-approval.md), which is implemented as the [Approval Voting](approval/approval-voting.md) subsystem. This system involves validators self-selecting to re-check candidates included in all observed forks of the relay chain as well as an algorithm for observing validators' statements about assignment and approval in order to determine which candidates, and thus blocks, are with high probability valid. The highest approved ancestor of a given block can be determined by querying the Approval Voting subsystem via the [`ApprovalVotingMessage::ApprovedAncestor`](../types/overseer-protocol.md#approval-voting) message.

Lastly, we refuse to finalize any block including a candidate for which we are aware of an ongoing dispute or of a dispute resolving against the candidate. The exact means of doing this has not been determined yet.
