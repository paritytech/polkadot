# Provisioner

Relay chain block authorship authority is governed by BABE and is beyond the scope of the Overseer and the rest of the subsystems. That said, ultimately the block author needs to select a set of backable parachain candidates and other consensus data, and assemble a block from them. This subsystem is responsible for providing the necessary data to all potential block authors.

A major feature of the provisioner: this subsystem is responsible for ensuring that parachain block candidates are sufficiently available before sending them to potential block authors.

## Provisionable Data

### Backable Candidates

The block author can choose 0 or 1 backable parachain candidates per parachain; the only constraint is that each backable candidate has the appropriate relay parent. However, the choice of a backable candidate must be the block author's; the provisioner must ensure that block authors are aware of all available backable candidates.

> TODO: "and their backings": why?

### Signed Bitfields

Signed bitfields are attestations from a particular validator about which candidates it believes are available.

> TODO: Are these actually included in the relay chain block, or just used to help decide whether a block is available and therefore a backable candidate?

### Misbehavior Reports

Misbehavior reports contain proof that a validator or set of validators has misbehaved; they consist of a proof of some kind of misbehavior: double-voting, being the minority vote in a disputed block's vote, etc. These cause dots to be slashed and must be included in the block.

> TODO: This problem is mentioned elsewhere, but how do we force the block author to include a misbehavior report if they don't like its effects, i.e. they are among the nodes which get slashed?

### Dispute Inherent

> TODO: Is this different from a misbehavior report? How?

## Protocol

Input: [`ProvisionerMessage`](/type-definitions.html#provisioner-message)

## Functionality

Use `StartWork` and `StopWork` to manage a set of jobs for relay-parents we might be building upon.
Forward all messages to corresponding job.

## Block Authorship Provisioning Job

Track all signed bitfields, all backable candidates received. Provide them to the `RequestBlockAuthorshipData` requester via the `response_channel`.
