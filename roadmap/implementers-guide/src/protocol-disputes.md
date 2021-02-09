# Disputes

Fast forward to [more detailed disputes requirments](./disputes-flow.md).

## Motivation

All blocks that end up on chain should be valid.

To ensure attempts, successful or not, of including
a block that is invalid in respect to the validation code, must therefore be handled in some way with the offenders being punished and the whistle blowers being rewarded. Disputes and their resolution are the formal process to resolve these situations.

At some point a validator claims that the `PoV` (proof of validity) - which was distributed with candidate block - for a certain block is invalid.

Now the dispute can be happening quite some time later than the inclusion, but also right during backing. As such the block is stored or more so its Reed-Solomon encoded erasure chunks, from which the
PoV can be reconstructed.

A reconstructed PoV can be verified with the defined verification code, that is valid during the session the block was included or backed.
If the block is invalid and there exists at least one backing vote and one validity challenging vote, a dispute exists.
The existence of a dispute is detected by a backing checker
or, if the block made it through backing stage, by an approval checker.
In either case, the validator casts and distributes its vote via means of gossip.

At this point the set of backing validators can not be trusted (since they voted for the block despite something being
fishy at the very least). On the other hand, one must also consider, the validator that blew the whistle has ulterior motives
to do so (i.e. it is controlled by a third party and wants to incur damage to itself).
In either way, there are malicious validators around.
As a consequence, all validators at the time of block backing, are being notified via broadcast of
the first pair of backing and challenging vote.
Validators that backed the candidate implicitly count as votes. Those validators are allowed to cast
a regular vote (a non-backing vote) as well, but it is generally not in their interest to vote both sides, since that would
advance the progress towards supermajority either way and have their bonds slashed.
If both votes lean in the same direction, i.e. both positive they are only counted as one.
Two opposing votes by the same validator would be equal to an attempted double vote and would be slashed accordingly.

All validators at block inclusion time are eligible to (and should) cast their Vote. The backing votes of backing checkers
are counted as votes as well.

## Initiation

A dispute is initiated by one approval checker creating and gossiping a vote, that challenges another vote.

After a backing or approval checker challenged a block, all validators that received the gossiped vote, reconstruct the block
from availability erasure code chunks and check the block's PoV themselves via the validation code.
The result of that check is converted into a vote, and distributed via the same mechanics as the first one.

Once a receiver receives ⅔ supermajority in one or the other direction, the
vote is concluded.
Conclusion implies that the result for this block can not be altered anymore, valid or invalid is fixed now.

In order to ensure, the dispute result is not forgotten or intentionally side stepped, it has to be recorded on chain.
This on chain recording mechanic must be vigilant, in a sense, that new emerging forks
must also receive the dispute resolution recorded (transplantation) irrespectively of the disputed block being included in that chain or not.

If the disputed block was already finalized, the chain must be put in governance mode to be resolved by human interaction
(i.e. sudo or motion or other mechanics that are available ).

As such the validator has to keep track of all votes irrespective if the disputed block is already known or not.
All backing votes should be either kept in storage as well, or be queried on demand, since they are a kind of vote
as well.

## Late votes

Late votes, after the dispute already reached a ⅔ supermajority, must be rewarded (albeit a smaller amount) as well.
These ones must be attached to the votes after a defined period of time after the result has reached
the required ⅔ supermajority.

## Chain Selection / Grandpa

Chain selection should be influenced by the chance of picking a chain that does not even include the disputed block.
Hence removing the need to include the dispute resolution itself.
This is only possible though, if the set of active heads contains such a fork.
In Grandpa the Voting rule should be used to avoid finalizing chains that contain an open or negative shut (shut with supermajority that marks the block as invalid) dispute.
In case all possible chains contains such a dispute, a metric must be used to decide which fork to use or avoid finalization until one dispute resolves positive (the
block is valid).
