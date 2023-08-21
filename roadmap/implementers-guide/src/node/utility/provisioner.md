# Provisioner

Relay chain block authorship authority is governed by BABE and is beyond the scope of the Overseer and the rest of the subsystems. That said, ultimately the block author needs to select a set of backable parachain candidates and other consensus data, and assemble a block from them. This subsystem is responsible for providing the necessary data to all potential block authors. 

## Provisionable Data

There are several distinct types of provisionable data, but they share this property in common: all should eventually be included in a relay chain block.

### Backed Candidates

The block author can choose 0 or 1 backed parachain candidates per parachain; the only constraint is that each backable candidate has the appropriate relay parent. However, the choice of a backed candidate must be the block author's. The provisioner subsystem is how those block authors make this choice in practice.

### Signed Bitfields

[Signed bitfields](../../types/availability.md#signed-availability-bitfield) are attestations from a particular validator about which candidates it believes are available. Those will only be provided on fresh leaves.

### Misbehavior Reports

Misbehavior reports are self-contained proofs of misbehavior by a validator or group of validators. For example, it is very easy to verify a double-voting misbehavior report: the report contains two votes signed by the same key, advocating different outcomes. Concretely, misbehavior reports become inherents which cause dots to be slashed.

Note that there is no mechanism in place which forces a block author to include a misbehavior report which it doesn't like, for example if it would be slashed by such a report. The chain's defense against this is to have a relatively long slash period, such that it's likely to encounter an honest author before the slash period expires.

### Dispute Inherent

The dispute inherent is similar to a misbehavior report in that it is an attestation of misbehavior on the part of a validator or group of validators. Unlike a misbehavior report, it is not self-contained: resolution requires coordinated action by several validators. The canonical example of a dispute inherent involves an approval checker discovering that a set of validators has improperly approved an invalid parachain block: resolving this requires the entire validator set to re-validate the block, so that the minority can be slashed.

Dispute resolution is complex and is explained in substantially more detail [here](../../runtime/disputes.md).

## Protocol

The subsystem should maintain a set of handles to Block Authorship Provisioning iterations that are currently live.

### On Overseer Signal

- `ActiveLeavesUpdate`:
  - For each `activated` head:
    - spawn a Block Authorship Provisioning iteration with the given relay parent, storing a bidirectional channel with that iteration.
  - For each `deactivated` head:
    - terminate the Block Authorship Provisioning iteration for the given relay parent, if any.
- `Conclude`: Forward `Conclude` to all iterations, waiting a small amount of time for them to join, and then hard-exiting.

### On `ProvisionerMessage`

Forward the message to the appropriate Block Authorship Provisioning iteration, or discard if no appropriate iteration is currently active.

### Per Provisioning Iteration

Input: [`ProvisionerMessage`](../../types/overseer-protocol.md#provisioner-message). Backed candidates come from the [Candidate Backing subsystem](../backing/candidate-backing.md), signed bitfields come from the [Bitfield Distribution subsystem](../availability/bitfield-distribution.md), and disputes come from the [Disputes Subsystem](../disputes/dispute-coordinator.md). Misbehavior reports are currently sent from the [Candidate Backing subsystem](../backing/candidate-backing.md) and contain the following misbehaviors:

1. `Misbehavior::ValidityDoubleVote`
2. `Misbehavior::MultipleCandidates`
3. `Misbehavior::UnauthorizedStatement`
4. `Misbehavior::DoubleSign`

But we choose not to punish these forms of misbehavior for the time being. Risks from misbehavior are sufficiently mitigated at the protocol level via reputation changes. Punitive actions here may become desirable enough to dedicate time to in the future.

At initialization, this subsystem has no outputs.

Block authors request the inherent data they should use for constructing the inherent in the block which contains parachain execution information.

## Block Production

When a validator is selected by BABE to author a block, it becomes a block producer. The provisioner is the subsystem best suited to choosing which specific backed candidates and availability bitfields should be assembled into the block. To engage this functionality, a `ProvisionerMessage::RequestInherentData` is sent; the response is a [`ParaInherentData`](../../types/runtime.md#parainherentdata). Each relay chain block backs at most one backable parachain block candidate per parachain. Additionally no further block candidate can be backed until the previous one either gets declared available or expired. If bitfields indicate that candidate A, predecessor of B, should be declared available, then B can be backed in the same relay block. Appropriate bitfields, as outlined in the section on [bitfield selection](#bitfield-selection), and any dispute statements should be attached as well.

### Bitfield Selection

Our goal with respect to bitfields is simple: maximize availability. However, it's not quite as simple as always including all bitfields; there are constraints which still need to be met:

- not more than one bitfield per validator
- each 1 bit must correspond to an occupied core

Beyond that, a semi-arbitrary selection policy is fine. In order to meet the goal of maximizing availability, a heuristic of picking the bitfield with the greatest number of 1 bits set in the event of conflict is useful.

### Dispute Statement Selection

This is the point at which the block author provides further votes to active disputes or initiates new disputes in the runtime state.

The block-authoring logic of the runtime has an extra step between handling the inherent-data and producing the actual inherent call, which we assume performs the work of filtering out disputes which are not relevant to the on-chain state. Backing votes are always kept in the dispute statement set. This ensures we punish the maximum number of misbehaving backers.

To select disputes:

- Issue a `DisputeCoordinatorMessage::RecentDisputes` message and wait for the response. This is a set of all disputes in recent sessions which we are aware of.

### Determining Bitfield Availability

An occupied core has a `CoreAvailability` bitfield. We also have a list of `SignedAvailabilityBitfield`s. We need to determine from these whether or not a core at a particular index has become available.

The key insight required is that `CoreAvailability` is transverse to the `SignedAvailabilityBitfield`s: if we conceptualize the list of bitfields as many rows, each bit of which is its own column, then `CoreAvailability` for a given core index is the vertical slice of bits in the set at that index.

To compute bitfield availability, then:

- Start with a copy of `OccupiedCore.availability`
- For each bitfield in the list of `SignedAvailabilityBitfield`s:
  - Get the bitfield's `validator_index`
  - Update the availability. Conceptually, assuming bit vectors: `availability[validator_index] |= bitfield[core_idx]`
- Availability has a 2/3 threshold. Therefore: `3 * availability.count_ones() >= 2 * availability.len()`

### Candidate Selection: Prospective Parachains Mode

The state of the provisioner `PerRelayParent` tracks an important setting, `ProspectiveParachainsMode`. This setting determines which backable candidate selection method the provisioner uses.

`ProspectiveParachainsMode::Disabled` - The provisioner uses its own internal legacy candidate selection. 
`ProspectiveParachainsMode::Enabled` - The provisioner requests that [prospective parachains](../backing/prospective-parachains.md) provide selected candidates.

Candidates selected with `ProspectiveParachainsMode::Enabled` are able to benefit from the increased block production time asynchronous backing allows. For this reason all Polkadot protocol networks will eventually use prospective parachains candidate selection. Then legacy candidate selection will be removed as obsolete.

### Prospective Parachains Candidate Selection

The goal of candidate selection is to determine which cores are free, and then to the degree possible, pick a candidate appropriate to each free core. In prospective parachains candidate selection the provisioner handles the former process while [prospective parachains](../backing/prospective-parachains.md) handles the latter.

To select backable candidates:

- Get the list of core states from the runtime API
- For each core state:
  - On `CoreState::Free`
    - The core is unscheduled and doesn’t need to be provisioned with a candidate
  - On `CoreState::Scheduled`
    - The core is unoccupied and scheduled to accept a backed block for a particular `para_id`.
    - The provisioner requests a backable candidate from [prospective parachains](../backing/prospective-parachains.md) with the desired relay parent, the core’s scheduled `para_id`, and an empty required path. 
  - On `CoreState::Occupied`
    - The availability core is occupied by a parachain block candidate pending availability. A further candidate need not be provided by the provisioner unless the core will be vacated this block. This is the case when either bitfields indicate the current core occupant has been made available or a timeout is reached.
    - If `bitfields_indicate_availability`
      - If `Some(scheduled_core) = occupied_core.next_up_on_available`, the core will be vacated and in need of a provisioned candidate. The provisioner requests a backable candidate from [prospective parachains](../backing/prospective-parachains.md) with the core’s scheduled `para_id` and a required path with one entry. This entry corresponds to the parablock candidate previously occupying this core, which was made available and can be built upon even though it hasn’t been seen as included in a relay chain block yet. See the Required Path section below for more detail.
      - If `occupied_core.next_up_on_available` is `None`, then the core being vacated is unscheduled and doesn’t need to be provisioned with a candidate.
    - Else-if `occupied_core.time_out_at == block_number`
      - If `Some(scheduled_core) = occupied_core.next_up_on_timeout`, the core will be vacated and in need of a provisioned candidate. A candidate is requested in exactly the same way as with `CoreState::Scheduled`.
      - Else the core being vacated is unscheduled and doesn’t need to be provisioned with a candidate
The end result of this process is a vector of `CandidateHash`s, sorted in order of their core index.

#### Required Path

Required path is a parameter for `ProspectiveParachainsMessage::GetBackableCandidate`, which the provisioner sends in candidate selection. 

An empty required path indicates that the requested candidate should be a direct child of the most recently included parablock for the given `para_id` as of the given relay parent. 

In contrast, a required path with one or more entries prompts [prospective parachains](../backing/prospective-parachains.md) to step forward through its fragment tree for the given `para_id` and relay parent until the desired parablock is reached. We then select a direct child of that parablock to pass to the provisioner. 

The parablocks making up a required path do not need to have been previously seen as included in relay chain blocks. Thus the ability to provision backable candidates based on a required path effectively decouples backing from inclusion.

### Legacy Candidate Selection 

Legacy candidate selection takes place in the provisioner. Thus the provisioner needs to keep an up to date record of all [backed_candidates](../../types/backing.md#backed-candidate) `PerRelayParent` to pick from. 

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

The end result of this process is a vector of `CandidateHash`s, sorted in order of their core index.

### Retrieving Full `BackedCandidate`s for Selected Hashes

Legacy candidate selection and prospective parachains candidate selection both leave us with a vector of `CandidateHash`s. These are passed to the backing subsystem with `CandidateBackingMessage::GetBackedCandidates`.

The response is a vector of `BackedCandidate`s, sorted in order of their core index and ready to be provisioned to block authoring. The candidate selection and retrieval process should select at maximum one candidate which upgrades the runtime validation code.

## Glossary

- **Relay-parent:** 
  - A particular relay-chain block which serves as an anchor and reference point for processes and data which depend on relay-chain state.
- **Active Leaf:** 
  - A relay chain block which is the head of an active fork of the relay chain. 
  - Block authorship provisioning jobs are spawned per active leaf and concluded for any leaves which become inactive.
- **Candidate Selection:** 
  - The process by which the provisioner selects backable parachain block candidates to pass to block authoring.
  - Two versions, prospective parachains candidate selection and legacy candidate selection. See their respective protocol sections for details.
- **Availability Core:** 
  - Often referred to simply as "cores", availability cores are an abstraction used for resource management. For the provisioner, availability cores are most relevant in that core states determine which `para_id`s to provision backable candidates for.
  - For more on availability cores see [Scheduler Module: Availability Cores](../../runtime/scheduler.md#availability-cores)
- **Availability Bitfield:**
  - Often referred to simply as a "bitfield", an availability bitfield represents the view of parablock candidate availability from a particular validator's perspective. Each bit in the bitfield corresponds to a single [availability core](../../runtime-api/availability-cores.md).
  - For more on availability bitfields see [availability](../../types/availability.md)
- **Backable vs. Backed:**
  - Note that we sometimes use "backed" to refer to candidates that are "backable", but not yet backed on chain.
  - Backable means that a quorum of the candidate's assigned backing group have provided signed affirming statements.