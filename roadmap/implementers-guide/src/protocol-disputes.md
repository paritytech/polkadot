# Disputes

Fast forward to [more detailed disputes requirments](./disputes-flow.md).

## Motivation and Background

All parachain blocks that end up in the finalized relay chain should be valid. This does not apply to blocks that are only backed, but not included.

We have two primary components for ensuring that nothing invalid ends up in the finalized relay chain:
  * Approval Checking, as described [here](./protocol-approval.md) and implemented according to the [Approval Voting](node/approval/approval-voting.md) subsystem. This protocol can be shown to prevent invalid parachain blocks from making their way into the finalized relay chain as long as the amount of attempts are limited.
  * Disputes, this protocol, which ensures that each attempt to include something bad is caught, and the offending validators are punished.

Disputes and their resolution are the formal process to resolve these situations.

Every dispute stems from a disagreement among two or more validators. If a bad actor creates a bad block, but the bad actor never distributes it to honest validators, then nobody will dispute it. Of course, such a situation is not even an attack on the network, so we don't need to worry about defending against it.

From most to least important, here are the attack scenarios we are interested in identifying and deterring:
  * A parablock included on a branch of the relay chain is bad
  * A parablock backed on a branch of the relay chain is bad
  * A parablock seconded, but not backed on any branch of the relay chain, is bad.

As covered in the [protocol overview](./protocol-overview.md), checking a parachain block requires 3 pieces of data: the parachain validation code, the [`AvailableData`](types/availability.md), and the [`CandidateReceipt`](types/candidate.md). The validation code is available on-chain, and published ahead of time, so that no two branches of the relay chain have diverging views of the validation code for a given parachain. Note that only for the first scenario, where the parablock has been included on a branch of the relay chain, is the data necessarily available. Thus, dispute processes should begin with an availability process to ensure availability of the `AvailableData`. This availability process will conclude quickly if the data is already available. If the data is not already available, then the initiator of the dispute must make it available.

Disputes have both an on-chain and an off-chain component. Slashing and punishment is handled on-chain, so votes by validators on either side of the dispute must be placed on-chain. Furthermore, a dispute on one branch of the relay chain should be transposed to all other active branches of the relay chain. The fact that slashing occurs _in all histories_ is crucial for deterring attempts to attack the network. The attacker should not be able to escape with their funds because the network has moved on to another branch of the relay chain where no attack was attempted.

In fact, this is why we introduce a distinction between _local_ and _remote_ disputes. We categorize disputes as either local or remote relative to any particular branch of the relay chain. Local disputes are about dealing with our first scenario, where a parablock has been included on the specific branch we are looking at. In these cases, the chain is corrupted all the way back to the point where the parablock was backed and must be discarded. However, as mentioned before, the dispute must propagate to all other branches of the relay chain. All other disputes are considered _remote_. For the on-chain component, when handling a dispute for a block which was not included in the current fork of the relay chain, it is impossible to discern between our attack scenarios. It is possible that the parablock was included somewhere, or backed somewhere, or wasn't backed anywhere. The on-chain component for handling these cases will be the same.

## Initiation

Disputes are initiated by any validator who finds their opinion on the validity of a parablock in opposition to another issued statement. As all statements currently gathered by the relay chain imply validity, disputes will be initiated only by nodes which perceive that the parablock is bad.

The initiation of a dispute begins off-chain. A validator signs a message indicating that it disputes the validity of the parablock and notifies all other validators, off-chain, of all of the statements it is aware of for the disputed parablock. These may be backing statements or approval-checking statements. It is worth noting that there is no special message type for initiating a dispute. It is the same message as is used to participate in a dispute and vote negatively. As such, there is no consensus required on who initiated a dispute, only on the fact that there is a dispute in-progress.

In practice, the initiator of a dispute will be either one of the backers or one of the approval checkers for the parablock. If the result of execution is found to be invalid, the validator will initiate the dispute as described above. Furthermore, if the dispute occurs during the backing phase, the initiator must make the data available to other validators. If the dispute occurs during approval checking, the data is already available.

Lastly, it is possible that for backing disputes, i.e. where the data is not already available among all validators, that an adversary may DoS the few parties who are checking the block to prevent them from distributing the data to other validators participating in the dispute process. Note that this can only occur pre-inclusion for any given parablock, so the downside of this attack is small and it is not security-critical to address these cases. However, we assume that the adversary can only prevent the validator from issuing messages for a limited amount of time. We also assume that there is a side-channel where the relay chain's governance mechanisms can trigger disputes by providing the full PoV and candidate receipt on-chain manually.

## Dispute Participation

Once becoming aware of a dispute, it is the responsibility of all validators to participate in the dispute. Concretely, this means:
  * Circulate all statements about the candidate that we are aware of - backing statements, approval checking statements, and dispute statements.
  * If we have already issued any type of statement about the candidate, go no further.
  * Download the [`AvailableData`](types/availability.md). If possible, this should first be attempted from other dispute participants or backing validators, and then [(via erasure-coding)](node/availability/availability-recovery.md) from all validators. 
  * Extract the Validation Code from any recent relay chain block. Code is guaranteed to be kept available on-chain, so we don't need to download any particular fork of the chain.
  * Execute the block under the validation code, using the `AvailableData`, and check that all outputs are correct, including the `erasure-root` of the [`CandidateReceipt`](types/candidate.md).
  * Issue a dispute participation statement to the effect of the validity of the candidate block.

Disputes _conclude_ after ⅔ supermajority is reached in either direction. 

The on-chain component of disputes can be initiated by providing any two conflicting votes and it also waits for a ⅔ supermajority on either side. The on-chain component also tracks which parablocks have already been disputed so the same parablock may only be disputed once on any particular branch of the relay chain. Lastly, it also tracks which blocks have been included on the current branch of the relay chain. When a dispute is initiated for a para, inclusion is halted for the para until the dispute concludes. 

The author of a relay chain block should initiate the on-chain component of disputes for all disputes which the chain is not aware of, and provide all statements to the on-chain component as well. This should all be done via _inherents_.

Validators can learn about dispute statements in two ways:
  * Receiving them from other validators over gossip
  * Scraping them from imported blocks of the relay chain. This is also used for validators to track other types of statements, such as backing statements.

Validators are rewarded for providing statements to the chain as well as for participating in the dispute, on either side. However, the losing side of the dispute is slashed.

## Dispute Conclusion

Disputes, roughly, are over when one side reaches a ⅔ supermajority. They may also conclude after a timeout, without either side witnessing supermajority, which will only happen if the majority of validators are unable to vote for some reason. Furthermore, disputes on-chain will stay open for some fixed amount of time even after concluding, to accept new votes.

Late votes, after the dispute already reached a ⅔ supermajority, must be rewarded (albeit a smaller amount) as well.

## Chain Selection / Grandpa

The [Approval Checking](protocol-approval.md) protocol prevents finalization of chains that contain parablocks that are not yet approved. With disputes, we take it one step further and do not vote to finalize any chains which contain parablocks that are being disputed or have lost a dispute anywhere.
