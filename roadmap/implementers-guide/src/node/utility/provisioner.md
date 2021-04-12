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

> TODO: The provisioner is responsible for selecting remote disputes to replay. Let's figure out the details.

## Protocol

Input: [`ProvisionerMessage`](../../types/overseer-protocol.md#provisioner-message). Backed candidates come from the [Candidate Backing subsystem](../backing/candidate-backing.md), signed bitfields come from the [Bitfield Distribution subsystem](../availability/bitfield-distribution.md), and misbehavior reports and disputes come from the [Misbehavior Arbitration subsystem](misbehavior-arbitration.md).

At initialization, this subsystem has no outputs.

Block authors request the inherent data they should use for constructing the inherent in the block which contains parachain execution information.

## Block Production

When a validator is selected by BABE to author a block, it becomes a block producer. The provisioner is the subsystem best suited to choosing which specific backed candidates and availability bitfields should be assembled into the block. To engage this functionality, a `ProvisionerMessage::RequestInherentData` is sent; the response is a [`ParaInherentData`](../../types/runtime.md#parainherentdata). There are never two distinct parachain candidates included for the same parachain and that new parachain candidates cannot be backed until the previous one either gets declared available or expired. Appropriate bitfields, as outlined in the section on [bitfield selection](#bitfield-selection), and any dispute statements should be attached as well.

### Bitfield Selection

Our goal with respect to bitfields is simple: maximize availability. However, it's not quite as simple as always including all bitfields; there are constraints which still need to be met:

- We cannot choose more than one bitfield per validator.
- Each bitfield must correspond to an occupied core.

Beyond that, a semi-arbitrary selection policy is fine. In order to meet the goal of maximizing availability, a heuristic of picking the bitfield with the greatest number of 1 bits set in the event of conflict is useful.

### Candidate Selection

The goal of candidate selection is to determine which cores are free, and then to the degree possible, pick a candidate appropriate to each free core.

To determine availability:

- Get the list of core states from the runtime API
- For each core state:
  - On `CoreState::Scheduled`, then we can make an `OccupiedCoreAssumption::Free`.
  - On `CoreState::Occupied`, then we may be able to make an assumption:
    - If the bitfields indicate availability and there is a scheduled `next_up_on_available`, then we can make an `OccupiedCoreAssumption::Included`.
    - If the bitfields do not indicate availability, and there is a scheduled `next_up_on_time_out`, and `occupied_core.time_out_at == block_number_under_production`, then we can make an `OccupiedCoreAssumption::TimedOut`.
  - If we did not make an `OccupiedCoreAssumption`, then continue on to the next core.
  - Now compute the core's `validation_data_hash`: get the `PersistedValidationData` from the runtime, given the known `ParaId` and `OccupiedCoreAssumption`;
  - Find an appropriate candidate for the core.
    - There are two constraints: `backed_candidate.candidate.descriptor.para_id == scheduled_core.para_id && candidate.candidate.descriptor.validation_data_hash == computed_validation_data_hash`.
    - In the event that more than one candidate meets the constraints, selection between the candidates is arbitrary. However, not more than one candidate can be selected per core.

The end result of this process is a vector of `BackedCandidate`s, sorted in order of their core index. Furthermore, this process should select at maximum one candidate which upgrades the runtime validation code.

### Dispute Statement Selection

This is the point at which the block author provides further votes to active disputes or initiates new disputes in the runtime state.

We must take care not to overwhelm the "spam slots" of the chain. That is, to avoid too many votes from the same validators being placed into the chain, which would trigger the anti-spam protection functionality of the [disputes module](../../runtime/disputes.md).

To select disputes:

- Make a `DisputesInfo` runtime API call and decompose into `{ spam_slots, disputes }`. Bind `disputes` to `onchain_disputes`.
- Issue a `DisputeCoordinatorMessage::ActiveDisputes` message and wait for the response. Assign the value to `offchain_disputes`.
- Make a `CandidatesIncluded` runtime API call for each dispute in `offchain_disputes` and tag each offchain dispute as local if the result for it is `true`.
- Initialize `NewSpamSlots: Map<(SessionIndex, ValidatorIndex), u32>` as an empty map.
- For each dispute in `offchain_disputes`:
  1. Make a `RuntimeApiRequest::SessionInfo` against the parent hash for the session of the dispute. If `None`, continue - this chain is in the past relative to the session the dispute belongs to and we can import it when it reaches that session.
  1. Load the spam slots from `spam_slots` for the given session. If it isn't present, treat as though all zeros.
  1. construct a `DisputeStatementSet` of all offchain votes we are aware of that the onchain doesn't already have a `valid` or `invalid` bit set for, respectively.
  1. If the `onchain_disputes` contains an entry for the dispute, load that. Otherwise, treat as empty.
  1. If the offchain dispute is local or the `DisputeStatementSet` and the onchain dispute together have at least `byzantine_threshold + 1` validators in it, continue to the next offchain dispute.
  1. Otherwise
    1. Filter out all votes from the `DisputeStatementSet` where the amount of spam slots occupied on-chain by the validator, plus the `NewSpamSlots` value, plus 1, is greater than `spam_slots.max_spam_slots`.
    1. After filtering, if either the `valid` or `invalid` lists in the combination of the `DisputeStatementSet` and the onchain dispute is empty, skip this dispute.
    1. Add 1 to the `NewSpamSlots` value for each validator in the `DisputeStatementSet`.
- Construct a `MultiDisputeStatementSet` for each `DisputeStatement` and return that.

### Determining Bitfield Availability

An occupied core has a `CoreAvailability` bitfield. We also have a list of `SignedAvailabilityBitfield`s. We need to determine from these whether or not a core at a particular index has become available.

The key insight required is that `CoreAvailability` is transverse to the `SignedAvailabilityBitfield`s: if we conceptualize the list of bitfields as many rows, each bit of which is its own column, then `CoreAvailability` for a given core index is the vertical slice of bits in the set at that index.

To compute bitfield availability, then:

- Start with a copy of `OccupiedCore.availability`
- For each bitfield in the list of `SignedAvailabilityBitfield`s:
  - Get the bitfield's `validator_index`
  - Update the availability. Conceptually, assuming bit vectors: `availability[validator_index] |= bitfield[core_idx]`
- Availability has a 2/3 threshold. Therefore: `3 * availability.count_ones() >= 2 * availability.len()`

### Notes

See also: [Scheduler Module: Availability Cores](../../runtime/scheduler.md#availability-cores).

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
