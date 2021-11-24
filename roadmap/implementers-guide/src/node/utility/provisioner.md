# Provisioner

Relay chain block authorship authority is governed by BABE and is beyond the scope of the Overseer and the rest of the subsystems. That said, ultimately the block author needs to select a set of backable parachain candidates and other consensus data, and assemble a block from them. This subsystem is responsible for providing the necessary data to all potential block authors.

A major feature of the provisioner: this subsystem is responsible for ensuring that parachain block candidates are sufficiently available before sending them to potential block authors.

## Provisionable Data

There are several distinct types of provisionable data, but they share this property in common: all should eventually be included in a relay chain block.

### Backed Candidates

The block author can choose 0 or 1 backed parachain candidates per parachain; the only constraint is that each backed candidate has the appropriate relay parent. However, the choice of a backed candidate must be the block author's; the provisioner must ensure that block authors are aware of all available [`BackedCandidate`s](../../types/backing.md#backed-candidate).

### Signed Bitfields

[Signed bitfields](../../types/availability.md#signed-availability-bitfield) are attestations from a particular validator about which candidates it believes are available.

### Misbehavior Reports

Misbehavior reports are self-contained proofs of misbehavior by a validator or group of validators. For example, it is very easy to verify a double-voting misbehavior report: the report contains two votes signed by the same key, advocating different outcomes. Concretely, misbehavior reports become inherents which cause dots to be slashed.

Note that there is no mechanism in place which forces a block author to include a misbehavior report which it doesn't like, for example if it would be slashed by such a report. The chain's defense against this is to have a relatively long slash period, such that it's likely to encounter an honest author before the slash period expires.

### Dispute Inherent

The dispute inherent is similar to a misbehavior report in that it is an attestation of misbehavior on the part of a validator or group of validators. Unlike a misbehavior report, it is not self-contained: resolution requires coordinated action by several validators. The canonical example of a dispute inherent involves an approval checker discovering that a set of validators has improperly approved an invalid parachain block: resolving this requires the entire validator set to re-validate the block, so that the minority can be slashed.

Dispute resolution is complex and is explained in substantially more detail [here](../../runtime/disputes.md).

## Protocol

Input: [`ProvisionerMessage`](../../types/overseer-protocol.md#provisioner-message). Backed candidates come from the [Candidate Backing subsystem](../backing/candidate-backing.md), signed bitfields come from the [Bitfield Distribution subsystem](../availability/bitfield-distribution.md), and misbehavior reports and disputes come from the [Misbehavior Arbitration subsystem](misbehavior-arbitration.md).

At initialization, this subsystem has no outputs.

Block authors request the inherent data they should use for constructing the inherent in the block which contains parachain execution information.

## Block Production

When a validator is selected by BABE to author a block, it becomes a block producer. The provisioner is the subsystem best suited to choosing which specific backed candidates and availability bitfields should be assembled into the block. To engage this functionality, a `ProvisionerMessage::RequestInherentData` is sent; the response is a [`ParaInherentData`](../../types/runtime.md#parainherentdata). There are never two distinct parachain candidates included for the same parachain and that new parachain candidates cannot be backed until the previous one either gets declared available or expired. Appropriate bitfields, as outlined in the section on [bitfield selection](#bitfield-selection), and any dispute statements should be attached as well.

### Bitfield Collection

Collects all provided bitfields and forwards them to the runtimes `fn create_inherent` which does both sanity (one bitfield per validator) as well as weight limiting logic.

### Backed Candidate Collection

Selects all referenced candidates the correspond to the
correct relay chain parent block and assures only one candidate per block is submitted.

The full backed candidate are obtained by issuing a `CandidateBackingMessage::GetBackedCandidates` which yields a `Vec<BackedCandidate>` in response.

The end result of this process is a vector of `BackedCandidate`s, sorted in the order of their `CandidateReceipt`s.

### Dispute Statement Selection

This is the point at which the block author provides further votes to active disputes or initiates new disputes in the runtime state.

To select disputes:

- Issue a `DisputeCoordinatorMessage::RecentDisputes` message and wait for the response. This is a set of all disputes in recent sessions which we are aware of. They are passed on unaltered to the `fn create_inherent` which is run off-chain.

## Functionality

The subsystem should maintain a set of handles to Block Authorship Provisioning Jobs that are currently live.

### On Overseer Signal

- `ActiveLeavesUpdate`:
  - For each `activated` head:
    - spawn a Block Authorship Provisioning Job with the given relay parent, storing a bidirectional channel with that job.
  - For each `deactivated` head:
    - terminate the Block Authorship Provisioning Job for the given relay parent, if any.
- `Conclude`: Forward `Conclude` to all jobs, waiting a small amount of time for them to join, and then hard-exiting.

### On `ProvisionerMessage`

Forward the message to the appropriate Block Authorship Provisioning Job, or discard if no appropriate job is currently active.

## Block Authorship Provisioning Job

Maintain the set of channels to block authors. On receiving provisionable data, send a copy over each channel.
