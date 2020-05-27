# Subsystems Overview

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

- **Various Subsystems**: requests to validate a particular parablock candidate

### Outbound Messages

Responses from the Candidate Validation subsystem always return to the requester

- **Various Subsystems**: a parablock candidate is valid, or not

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

The misbehavior arbitration subsystem kicks in when two validator nodes disagree about a candidate's validity. In this case, _all_ validators, not just those assigned to its parachain, weigh in on the validity of this candidate. The minority is slashed.

### Incoming Messages

- **Statement Distribution**: Two validator nodes disgree on this candidate's validity, please figure it out
- **Statement Distribution**: I noticed a validator contradicting itself about this candidate's validity, please figure it out

### Outgoing Messages

- **Overseer**: the majority of nodes agree that this candidate is valid/invalid; here is a list of minority voters to slash

---

## Tracing

Let's follow a parachain candidate through the system. This follows the happy path, but we'll note exit points along the way.

- Collator (external to this system)
- Overseer
- Candidate Selection: every parachain validator can choose at most 1 candidate to back, so potentially many are discarded.
- Candidate Backing
- Statement Distribution: if there's disagreement about a candidate's validity, things move to misbehavior arbitration.
- Overseer: a quorum of validators unanimously agrees about this candidate's validity.
