# Prospective Parachains

## Overview

**Purpose:** Tracks and handles prospective parachain fragments and informs
other backing-stage subsystems of work to be done.

"prospective":
- [*prə'spɛktɪv*] adj.
- future, likely, potential

Asynchronous backing changes the runtime to accept parachain candidates from a
certain allowed range of historic relay-parents. This means we can now build
*prospective parachains* – that is, trees of potential (but likely) future
parachain blocks. This is the subsystem responsible for doing so.

Other subsystems such as Backing rely on Prospective Parachains, e.g. for
determining if a candidate can be seconded. This subsystem is the main
coordinator of work within the node for the collation and backing phases of
parachain consensus.

Prospective Parachains is primarily an implementation of fragment trees. It also
handles concerns such as:

- the relay-chain being forkful
- session changes

See the following sections for more details.

### Fragment Trees

This subsystem builds up fragment trees, which are trees of prospective para
candidates. Each path through the tree represents a possible state transition
path for the para. Each potential candidate is a fragment, or a node, in the
tree. Candidates are validated against constraints as they are added.

This subsystem builds up trees for each relay-chain block in the view, for each
para. These fragment trees are used for:

- providing backable candidates to other subsystems
- sanity-checking that candidates can be seconded
- getting seconded candidates under active leaves
- etc.

For example, here is a tree with several possible paths:

```
Para Head registered by the relay chain:     included_head
                                                  ↲  ↳
depth 0:                                  head_0_a    head_0_b
                                             ↲            ↳
depth 1:                             head_1_a              head_1_b
                                  ↲      |     ↳
depth 2:                 head_2_a1   head_2_a2  head_2_a3
```

### The Relay-Chain Being Forkful

We account for the same candidate possibly appearing in different forks. While
we still build fragment trees for each head in each fork, we are efficient with
how we reference candidates to save space.

### Session Changes

Allowed ancestry doesn't cross session boundary. That is, you can only build on
top of the freshest relay parent when the session starts. This is a current
limitation that may be lifted in the future.

Also, runtime configuration values needed for constraints (such as
`max_pov_size`) are constant within a session. This is important when building
prospective validation data. This is unlikely to change.

## Messages

### Incoming

- `ActiveLeaves`
  - Notification of a change in the set of active leaves.
  - Constructs fragment trees for each para for each new leaf.
- `ProspectiveParachainsMessage::IntroduceCandidate`
  - Informs the subsystem of a new candidate.
  - Sent by the Backing Subsystem when it is importing a statement for a
    new candidate.
- `ProspectiveParachainsMessage::CandidateSeconded`
  - Informs the subsystem that a previously introduced candidate has
    been seconded.
  - Sent by the Backing Subsystem when it is importing a statement for a
    new candidate after it sends `IntroduceCandidate`, if that wasn't
    rejected by Prospective Parachains.
- `ProspectiveParachainsMessage::CandidateBacked`
  - Informs the subsystem that a previously introduced candidate has
    been backed.
  - Sent by the Backing Subsystem after it successfully imports a
    statement giving a candidate the necessary quorum of backing votes.
- `ProspectiveParachainsMessage::GetBackableCandidate`
  - Get a backable candidate hash along with its relay parent for a given parachain,
    under a given relay-parent (leaf) hash, which is a descendant of given candidate hashes.
  - Sent by the Provisioner when requesting backable candidates, when
    selecting candidates for a given relay-parent.
- `ProspectiveParachainsMessage::GetHypotheticalFrontier`
  - Gets the hypothetical frontier membership of candidates with the
    given properties under the specified active leaves' fragment trees.
  - Sent by the Backing Subsystem when sanity-checking whether a candidate can
    be seconded based on its hypothetical frontiers.
- `ProspectiveParachainsMessage::GetTreeMembership`
  - Gets the membership of the candidate in all fragment trees.
  - Sent by the Backing Subsystem when it needs to update the candidates
    seconded at various depths under new active leaves.
- `ProspectiveParachainsMessage::GetMinimumRelayParents`
  - Gets the minimum accepted relay-parent number for each para in the
    fragment tree for the given relay-chain block hash.
  - That is, this returns the minimum relay-parent block number in the
    same branch of the relay-chain which is accepted in the fragment
    tree for each para-id.
  - Sent by the Backing, Statement Distribution, and Collator Protocol
    subsystems when activating leaves in the implicit view.
- `ProspectiveParachainsMessage::GetProspectiveValidationData`
  - Gets the validation data of some prospective candidate. The
    candidate doesn't need to be part of any fragment tree.
  - Sent by the Collator Protocol subsystem (validator side) when
    handling a fetched collation result.

### Outgoing

- `RuntimeApiRequest::StagingParaBackingState`
  - Gets the backing state of the given para (the constraints of the para and
    candidates pending availability).
- `RuntimeApiRequest::AvailabilityCores`
  - Gets information on all availability cores.
- `ChainApiMessage::Ancestors`
  - Requests the `k` ancestor block hashes of a block with the given
    hash.
- `ChainApiMessage::BlockHeader`
  - Requests the block header by hash.

## Glossary

- **Candidate storage:** Stores candidates and information about them
  such as their relay-parents and their backing states. Is indexed in
  various ways.
- **Constraints:**
  - Constraints on the actions that can be taken by a new parachain
    block.
  - Exhaustively define the set of valid inputs and outputs to parachain
    execution.
- **Fragment:** A prospective para block (that is, a block not yet referenced by
  the relay-chain). Fragments are anchored to the relay-chain at a particular
  relay-parent.
- **Fragment tree:**
  - A tree of fragments. Together, these fragments define one or more
    prospective paths a parachain's state may transition through.
  - See the "Fragment Tree" section.
- **Inclusion emulation:** Emulation of the logic that the runtime uses
  for checking parachain blocks.
- **Relay-parent:** A particular relay-chain block that a fragment is
  anchored to.
- **Scope:** The scope of a fragment tree, defining limits on nodes
  within the tree.
