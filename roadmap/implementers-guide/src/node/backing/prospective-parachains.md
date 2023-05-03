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

This subsystem builds up `FragmentTree`s, which are trees of prospective para
candidates. Each path through the tree represents a possible state transition
path for the para. Each potential candidate is a fragment, or a node, in the
tree. Candidates are validated against constraints as they are added.

This subsystem builds up trees for each relay-chain block in the view, for each
para. These `FragmentTree`s are used for:

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

### Fragment Trees Example

In this example we will step through the creation of a forked relay chain state 
with two `FragmentTree`s corresponding to the two relay chain active leaves. 
The `FragmentTree`s in this example will demonstrate two important criteria for 
a parablock candidate's inclusion in a tree:

  1. The parablock candidate can only be included in a `FragmentTree` as a 
  child of a fragment corresponding to its required parent. If no such parent 
  fragment exists, then the candidate is not included. See glossary for more 
  detail on required parent.
  2. The parablock candidate's relay parent, a reference point for the relay 
  chain context in which the candidate was produced, must be in scope. Two 
  examples of out-of-scope relay parents are those on a different fork of the 
  relay chain and those older than `allowed_ancestry_len`.
  An exception to this rule is made for parablocks pending availability. Such 
  parablocks have been seen on-chain and need to be accounted for even if they 
  go out of scope. The most likely outcome for candidates pending availability 
  is that they will become available, so we need those blocks to be in the 
  `FragmentTree` to accept their children.

This example also demonstrates how the same parablock candidate can be added to 
multiple `FragmentTree`s, even if those trees are rooted at different relay 
block heights. The appearance of the same candidate in more than one 
`FragmentTree` motivates the use of a single `CandidateStorage` instance which 
all trees reference.

Note the important difference between relay parent and required parent. The 
former refers to a relay chain block and the latter refers to an ancestor 
parachain block. See glossary for more detail.

#### Example Notation

The following symbolic notation will be used for the observed prospective 
parachains state at node _N_ for parachain _P_. Capital letters correspond to 
relay chain blocks, while lower case correspond to parablock candidates. 

Notation for a backable candidate for parachain _P_:
- backable candidate{relay parent, required parent}
- EX: `b{A,a}` A parablock candidate _b_ with relay parent _A_ and required 
parent _a_.

Notation for a relay block potentially backing one candidate and including 
another for parachain _P_:
- Relay block(backed candidate{relay parent, required parent}, included candidate)
- EX: `B(b{A,a},a)` A relay block _B_ which backs parablock _b_ and includes 
parablock _a_.

Underscores, _ , are used in two situations:
1. To omit relay parent or required parent information for a parablock candidate 
which isn't needed for our example.
2. An empty backed candidate or included candidate field in a relay chain block.

Arrows indicate ancestry relationships:
- EX: `A(_,_) -> B(_,_)` Relay block _A_ is the ancestor of block _B_

#### Fragment Trees Example, Step by Step

Prospective parachains state progression for parachain _P_ on node _N_ given a 
hypothetical series of candidate backing and inclusion events:

**1.** Parablock candidate _a_ generated and distributed to block producers as 
backable. For this example we don’t care about _a_’s relay parent and required 
parent. 

Resulting state: 
```
... -> a{_,_}
```

**2.** Relay block _A_ is imported, backing candidate _a_. For some time _A_ is an 
active leaf, and prospective parachains maintains a `FragmentTree` for parachain 
_P_ with root _A_. This tree is dropped in step 4 when _A_ ceases to be an 
active leaf.

Resulting state: 
```
... -> A(a{_,_},_)
```

**3.** Parablock candidate _b_ with relay parent _A_ and required parent _a_ is 
generated and distributed to block producers as backable. At this point 
prospective parachains for node _N_ has a single fragment tree with root _A_ and 
a single child node, _b_.

Resulting state: 
```
... -> A(a{_,_},_) -> b{A,a}
```

**4.** Relay block _B_ is imported as descendant of _A_, marking candidate _a_ as 
included and backing candidate _b_. A `FragmentTree` is created for parachain 
_P_ with root _B_. The `FragmentTree` with root _A_ is discarded.

Resulting state: 
```
... -> A(a{_,_},_) -> B(b{A,a},a)
```

**5.** Relay block _C_ is imported as descendant of _A_, forking the relay chain and 
marking candidate _a_ as included. This block doesn’t back any candidate for 
parachain _P_, conceivably because the block producer wasn’t aware of a backable 
candidate. A `FragmentTree` is created for parachain _P_ with root _C_. This 
tree is dropped in step 6 when _C_ ceases to be an active leaf.

Resulting state: 
```
... -> A(a{_,_},_) -> B(b{A,a},a)

                   -> C(_,a)
```

**6.** Relay block _D_ is imported as descendant of _C_, backing candidate _b_. A 
`FragmentTree` is created for parachain _P_ with root _D_. The `FragmentTree` 
with root _C_ is discarded.

Resulting state: 
```
... -> A(a{_,_},_) -> B(b{A,a},a)

                   -> C(_,a)      -> D(b{A,a},_)
```

**7.** Parablock candidate _c_ with relay parent _A_ and required parent _b_ is 
generated and distributed to block producers as backable. The `FragmentTree`s 
with roots _B_ and _D_ both contain a node corresponding to _c_’s required 
parent, _b_, so _c_ can be included in both trees as a child of _b_. 

Resulting state: 
```
... -> A(a{_,_},_) -> B(b{A,a},a) -> c{A,b}
                                    
                   -> C(_,a)      -> D(b{A,a},_) -> c{A,b}
```

**8.** Here candidates in the forks corresponding to relay blocks _B_ and _D_ 
begin to diverge for parachain _P_. A parablock candidate _d_ with relay 
parent _B_ and required parent _b_ is generated and distributed to block 
producers as backable. Candidate _d_’s relay parent, _B_, isn’t in scope for 
the `FragmentTree` with root _D_, since _B_ is on a different fork of the relay 
chain. Thus candidate _d_ can be added to the `FragmentTree` with root _B_, but 
not that with root _D_.

Resulting state: 
```
... -> A(a{_,_},_) -> B(b{A,a},a) -> c{A,b} 
                                  -> d{B,b}
                                    
                   -> C(_,a)      -> D(b{A,a},_) -> c{A,b}
```

**9.** A parablock candidate _e_ with relay parent _B_ and required parent _c_ is 
generated and distributed to block producers as backable. For the same reason 
as with candidate _d_, _e_ can only be added to the `FragmentTree` with root 
_B_.

Resulting state: 
```
... -> A(a{_,_},_) -> B(b{A,a},a) -> c{A,b} -> e{B,c}
                                  -> d{B,b}
                                    
                   -> C(_,a)      -> D(b{A,a},_) -> c{A,b}
```


#### Fragment Trees Example, Examining Final State

The above representation of prospective parachains state progressions is useful 
for conceptual continuity but doesn't reflect the actual data structures 
involved. Here is a look at the final state of our example represented as a 
shared `CandidateStore` and two `FragmentTrees`, each with its own 
`relay_parent` and `ancestors`. 

Note that the notation `b{A,a}` has different meaning in the `CandidateStore` 
than in a `FragmentTree`. In the `CandidateStore` it represents the candidate 
_b_. But in a `FragmentTree` it represents a node corresponding to 
candidate _b_ and pointing to _b_ in the `CandidateStore`.

**CandidateStore**:
```
a{_,_} , b{A,a} , c{A, b} , d{B, b} , e{B,c}
```

**Tree with root B:**
```
b{A,a} -> c{A,b} -> e{B,c}
       -> d{B,b}
```

`relay_parent` for tree _B_: `B(b{A,a},a)`

`ancestors` for tree _B_: `... , A(a{_,_},_)`

**Tree with root D:**
```
b{A,a} -> c{A,b}
```

`relay_parent` for tree _D_: `D(b{A,a},_)`

`ancestors` for tree _D_: `... , A(a{_,_}_) , C(_,a)`

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
  - Get a backable candidate hash for a given parachain, under a given
    relay-parent hash, which is a descendant of given candidate hashes.
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
- **Required parent:** Each parachain block candidate is built in the context of its most recent ancestor. In prospective parachains this relationship is used as a constraint so that fragments can only be added to the fragment tree as children of fragments corresponding to their most recent ancestor. The term required parent is given to this constraint.
- **Scope:** The scope of a fragment tree, defining limits on nodes
  within the tree.
