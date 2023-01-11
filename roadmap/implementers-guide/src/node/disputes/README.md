# Disputes Subsystems

This section is for the node-side subsystems that lead to participation in disputes. There are five major roles that validator nodes must participate in when it comes to disputes
    * Detection. Detect bad parablocks, either during candidate backing or approval checking, and initiate a dispute.
    * Participation. Participate in active disputes. When a node becomes aware of a dispute, it should recover the data for the disputed block, check the validity of the parablock, and issue a statement regarding the validity of the parablock.
    * Distribution. Validators should notify each other of active disputes and relevant statements over the network.
    * Submission. When authoring a block, a validator should inspect the state of the parent block and provide any information about disputes that the chain needs as part of the `ParaInherent`. This should initialize new disputes on-chain as necessary.
    * Fork-choice and Finality. When observing a block issuing a `DisputeRollback` digest in the header, that branch of the relay chain should be abandoned all the way back to the indicated block. When voting on chains in GRANDPA, no chains that contain blocks with active disputes or disputes that concluded invalid should be voted on.

## Components

### Dispute Coordinator

This component is responsible for coordinating other subsystems around disputes.

This component should track all statements received from all validators over some window of sessions. This includes backing statements, approval votes, and statements made specifically for disputes. This will be used to identify disagreements or instances where validators conflict with themselves.

This is responsible for tracking and initiating disputes. Disputes will be initiated either externally by another subsystem which has identified an issue with a parablock or internally upon observing two statements which conflict with each other on the validity of a parablock.

No more than one statement by each validator on each side of the dispute needs to be stored. That is, validators are allowed to participate on both sides of the dispute, although we won't write code to do so. Such behavior has negative extractable value in the runtime.

This will notify the dispute participation subsystem of a new dispute if the local validator has not issued any statements on the disputed candidate already.

Locally authored statements related to disputes will be forwarded to the dispute distribution subsystem.

This subsystem also provides two further behaviors for the interactions between disputes and fork-choice
    - Enhancing the finality voting rule. Given description of a chain and candidates included at different heights in that chain, it returns the `BlockHash` corresponding to the highest `BlockNumber` that there are no disputes before. I expect that we will slightly change `ApprovalVoting::ApprovedAncestor` to return this set and then the target block to vote on will be further constrained by this function.
    - Chain roll-backs. Whenever importing new blocks, the header should be scanned for a roll-back digest. If there is one, the chain should be rolled back according to the digest. I expect this would be implemented with a `ChainApi` function and possibly an `ApprovalVoting` function to clean up the approval voting DB.

### Dispute Participation

This subsystem ties together the dispute tracker, availability recovery, candidate validation, and dispute distribution subsystems. When notified of a new dispute by the Dispute Tracker, the data should be recovered, the validation code loaded from the relay chain, and the candidate is executed.

A statement depending on the outcome of the execution is produced, signed, and imported to the dispute tracker.

### Dispute Distribution

This is a networking component by which validators notify each other of live disputes and statements on those disputes.

Validators will in the future distribute votes to each other via the network, but at the moment learn of disputes just from watching the chain.
