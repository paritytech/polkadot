# Approval Subsystems

The approval subsystems implement the node-side of the [Approval Protocol](../../protocol-approval.md).

We make a divide between the [assignment/voting logic](approval-voting.md) and the [distribution logic](approval-distribution.md) that distributes assignment certifications and approval votes. The logic in the assignment and voting also informs the GRANDPA voting rule on how to vote.

These subsystems are intended to flag issues and begin participating in live disputes. Dispute subsystems also track all observed votes (backing, approval, and dispute-specific) by all validators on all candidates.
