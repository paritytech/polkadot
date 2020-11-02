# Approval Subsystems

The approval subsystems implement the node-side of the [Approval Protocol](../../protocol-approval.md).

We make a divide between the [assignment/voting logic](approval-voting.md) and the [networking](approval-networking.md) that distributes assignment certifications and approval votes. The logic in the assignment and voting also informs the GRANDPA voting rule on how to vote.

This category of subsystems also contains a module for [participating in live disputes](dispute-participation.md) and tracks all observed votes (backing or approval) by all validators on all candidates.