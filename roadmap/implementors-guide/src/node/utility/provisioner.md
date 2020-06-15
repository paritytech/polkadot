# Provisioner

Relay chain block authorship authority is governed by BABE and is beyond the scope of the Overseer and the rest of the subsystems. That said, ultimately the block author needs to select a set of backable parachain candidates and other consensus data, and assemble a block from them. This subsystem is responsible for providing the necessary data to all potential block authors.

A major feature of the provisioner: this subsystem is responsible for ensuring that parachain block candidates are sufficiently available before sending them to potential block authors.

## Provisionable Data

There are several distinct types of provisionable data, but they share this property in common: all should eventually be included in a relay chain block.

### Backed Candidates

The block author can choose 0 or 1 backed parachain candidates per parachain; the only constraint is that each backed candidate has the appropriate relay parent. However, the choice of a backed candidate must be the block author's; the provisioner must ensure that block authors are aware of all available [`BackedCandidate`s](/type-definitions.html#backed-candidate).

### Signed Bitfields

[Signed bitfields](/type-definitions.html#signed-availability-bitfield) are attestations from a particular validator about which candidates it believes are available.

### Misbehavior Reports

Misbehavior reports are self-contained proofs of misbehavior by a validator or group of validators. For example, it is very easy to verify a double-voting misbehavior report: the report contains two votes signed by the same key, advocating different outcomes. Concretely, misbehavior reports become inherents which cause dots to be slashed.

Note that there is no mechanism in place which forces a block author to include a misbehavior report which it doesn't like, for example if it would be slashed by such a report. The chain's defense against this is to have a relatively long slash period, such that it's likely to encounter an honest author before the slash period expires.

### Dispute Inherent

The dispute inherent is similar to a misbehavior report in that it is an attestation of misbehavior on the part of a validator or group of validators. Unlike a misbehavior report, it is not self-contained: resolution requires coordinated action by several validators. The canonical example of a dispute inherent involves a fisherman discovering that a set of validators has improperly approved an invalid parachain block: resolving this requires the entire validator set to re-validate the block, so that the minority can be slashed.

> TODO: Is this statement accurate: "Resolution of a dispute produces a misbehavior report."?

## Protocol

Input: [`ProvisionerMessage`](/type-definitions.html#provisioner-message)

## Functionality

Use `StartWork` and `StopWork` to manage a set of jobs for relay-parents we might be building upon.
Forward all messages to corresponding job.

## Block Authorship Provisioning Job

Track all signed bitfields, all backable candidates received. Provide them to the `RequestBlockAuthorshipData` requester via the `response_channel`.
