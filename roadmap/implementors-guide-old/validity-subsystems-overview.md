# Validity Subsystems Overview

These subsystems comprise the Validity module. All detailed documentation lives within the guide; this document simply attempts to enumerate and briefly describe each subsystem and the map of their communications.

- Overseer
- Candidate Selection
- Candidate Validation
- Candidate Backing
- Statement Distribution
- Misbehavior Arbitration

## Overseer

The overseer is the postal service: it relays messages between the other subsystems. At any time in this document when we state that a subsystem sends a message to another, it does so via the overseer. The overseer also alerts other subsystems when they should be prepared to work on new relay parents, and when they should shut down preparations to work on old, finalized, or otherwise obsolete relay parents.

## Candidate Selection

The candidate selection subsystem is in charge of seconding 0 or 1 parablock candidates per relay parent.

### Inbound Messages

- **Overseer**: a new parablock candidate has arrived from a collator.

### Outbound Messages

- **Candidate Backing**: this candidate (by hash) satisfies system properties; please validate it

## Candidate Backing

The candidate backing subsystem is in charge of producing statements which assert the (non-)validity of a parachain candidate.

### Inbound Messages

- **Candidate Selection**: a candidate which satisfies system properties
- **Candidate Validation**: a particular candidate is valid, or not

### Outbound Messages

- **Candidate Validation**: is this candidate in fact valid?
- **Statement Distribution**: I testify that this candidate is (in-)valid

## Candidate Validation

The candidate validation subsystem handles the details of validating a candidate. For any given parachain candidate, it arranges the appropriate context: the validation function, the relay parent head data, the in-progress relay data, etc.

### Inbound Messages

Note that the inbound message will only actually include the PoV when we are deciding whether or not to second a particular parachain candidate. In all other cases, we need to fetch it, because we only actually have the candidate receipt.

The candidate validation subsystem is required to keep a cache of parablock hashes to validation results; it should only ever actually perform validation for a given candidate once, no matter how many times the validity for that block is queried. When validation is performed, if the PoV is not included in the request, a new message will be dispatched to the overseer to fetch the appropriate PoV from the network.

To prevent unbounded growth, when the candidate validation subsystem is notified by the overseer to stop work on a particular block, it should clear the cache of results pertaining to that block.

- **Various Subsystems**: requests to validate a particular parablock candidate
- **Overseer**: response to PoV request

### Outbound Messages

Responses from the Candidate Validation subsystem always return to the requester

- **Various Subsystems**: a parablock candidate is valid, or not
- **Overseer**: request the PoV for a candidiate with a particular hash

## Statement Distribution

The statement distribution subsystem sends statements to peer nodes, detects double-voting, and tabulates when a sufficient portion of the validator set has unanimously judged a candidate. When judgment is not unanimous, it escalates the issue to misbehavior arbitration.

### Inbound Messages

- **Overseer**: a peer node has seconded a candidate

### Outbound Messages

- **Candidate Validation**: double-check this peer's seconded candidate
- **Peer validators**: Here's what I think about a candidate's validity
- **Misbehavior Arbitration**: I disagree with a peer node about this candidate's validity
- **Overseer**: a unanimous quorum of nodes has agreed about this candidate's validity

## Misbehavior Arbitration

The misbehavior arbitration subsystem kicks in when two validator nodes disagree about a candidate's validity. It is currently minimally specified pending future work.

### Incoming Messages

Note: this section is likely to change in a future PR.

- **Statement Distribution**: Two validator nodes disgree on this candidate's validity, please figure it out
- **Statement Distribution**: I noticed a validator contradicting itself about this candidate's validity, please figure it out

### Outgoing Messages

Note: this section is likely to change in a future PR.

- **Overseer**: the majority of nodes agree that this candidate is valid/invalid; here is a list of minority voters to slash

---

## Collators

Collators aren't strictly part of the Validity module, but they form an integral part of its workflow. Collators are associated with a particular parachain and understand it well enough to make validity assertions backed by their own stake.

In principle, there is not necessarily any relation between parachain block producers and collators: the block producer(s) just need to get unvalidated blocks to a collator in some parachain-specific way. In practice, we expect that much of the time, a collator _is_ a block producer for that parachain. Polkadot doesn't care, and can't tell, either way. What's important about a collator from Polkadot's perspective is that it can provide new blocks, and appropriate Proofs of Validity.

Once a Proof of Validity is available for a parachain block candidate, both are packaged together and handed to Polkadot. They make their way to the appropriate validators with substantial hand-waving about the details, and are deposited into the Validity module by way of the Overseer: as message broker, that subsystem is also the point of contact for the external world. The overseer then ensures that these are forwarded appropriately through the rest of the subsystems.

---

## Tracing

Let's follow a parachain candidate through the system. This follows the happy path, but we'll note exit points along the way.

- Collator (external to this system)
- Overseer: the overseer receives the message as the central message bus and point of contract to the outside world.
- Candidate Selection: every parachain validator can choose at most 1 candidate to back, so potentially many are discarded.
- Candidate Backing
- Statement Distribution: if there's disagreement about a candidate's validity, things move to misbehavior arbitration.
- Overseer: a quorum of validators unanimously agrees about this candidate's validity. Note: the overseer is not the terminal destination of the candidate at this point; instead, the overseer is responsible for handing it off to a separate module for block authorship.
