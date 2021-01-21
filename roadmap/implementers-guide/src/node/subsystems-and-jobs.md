# Subsystems and Jobs

In this section we define the notions of Subsystems and Jobs. These are guidelines for how we will employ an architecture of hierarchical state machines. We'll have a top-level state machine which oversees the next level of state machines which oversee another layer of state machines and so on. The next sections will lay out these guidelines for what we've called subsystems and jobs, since this model applies to many of the tasks that the Node-side behavior needs to encompass, but these are only guidelines and some Subsystems may have deeper hierarchies internally.

Subsystems are long-lived worker tasks that are in charge of performing some particular kind of work. All subsystems can communicate with each other via a well-defined protocol. Subsystems can't generally communicate directly, but must coordinate communication through an [Overseer](overseer.md), which is responsible for relaying messages, handling subsystem failures, and dispatching work signals.

Most work that happens on the Node-side is related to building on top of a specific relay-chain block, which is contextually known as the "relay parent". We call it the relay parent to explicitly denote that it is a block in the relay chain and not on a parachain. We refer to the parent because when we are in the process of building a new block, we don't know what that new block is going to be. The parent block is our only stable point of reference, even though it is usually only useful when it is not yet a parent but in fact a leaf of the block-DAG expected to soon become a parent (because validators are authoring on top of it). Furthermore, we are assuming a forkful blockchain-extension protocol, which means that there may be multiple possible children of the relay-parent. Even if the relay parent has multiple children blocks, the parent of those children is the same, and the context in which those children is authored should be the same. The parent block is the best and most stable reference to use for defining the scope of work items and messages, and is typically referred to by its cryptographic hash.

Since this goal of determining when to start and conclude work relative to a specific relay-parent is common to most, if not all subsystems, it is logically the job of the Overseer to distribute those signals as opposed to each subsystem duplicating that effort, potentially being out of synchronization with each other. Subsystem A should be able to expect that subsystem B is working on the same relay-parents as it is. One of the Overseer's tasks is to provide this heartbeat, or synchronized rhythm, to the system.

The work that subsystems spawn to be done on a specific relay-parent is known as a job. Subsystems should set up and tear down jobs according to the signals received from the overseer. Subsystems may share or cache state between jobs.

Subsystems must be robust to spurious exits. The outputs of the set of subsystems as a whole comprises of signed messages and data committed to disk. Care must be taken to avoid issuing messages that are not substantiated. Since subsystems need to be safe under spurious exits, it is the expected behavior that an `OverseerSignal::Conclude` can just lead to breaking the loop and exiting directly as opposed to waiting for everything to shut down gracefully.

## Subsystem Message Traffic

Which subsystems send messages to which other subsystems.

**Note**: This diagram omits the overseer for simplicity. In fact, all messages are relayed via the overseer.

**Note**: Messages with a filled diamond arrowhead ("♦") include a `oneshot::Sender` which communicates a response from the recipient.
Messages with an open triangle arrowhead ("Δ") do not include a return sender.

```dot process
digraph {
    rankdir=LR;
    node [shape = oval];
    concentrate = true;

    av_store    [label = "Availability Store"]
    avail_dist  [label = "Availability Distribution"]
    avail_rcov  [label = "Availability Recovery"]
    bitf_dist   [label = "Bitfield Distribution"]
    bitf_sign   [label = "Bitfield Signing"]
    cand_back   [label = "Candidate Backing"]
    cand_sel    [label = "Candidate Selection"]
    cand_val    [label = "Candidate Validation"]
    chn_api     [label = "Chain API"]
    coll_gen    [label = "Collation Generation"]
    coll_prot   [label = "Collator Protocol"]
    net_brdg    [label = "Network Bridge"]
    pov_dist    [label = "PoV Distribution"]
    provisioner [label = "Provisioner"]
    runt_api    [label = "Runtime API"]
    stmt_dist   [label = "Statement Distribution"]

    av_store    -> runt_api     [arrowhead = "diamond", label = "Request::CandidateEvents"]
    av_store    -> chn_api      [arrowhead = "diamond", label = "BlockNumber"]
    av_store    -> chn_api      [arrowhead = "diamond", label = "BlockHeader"]
    av_store    -> runt_api     [arrowhead = "diamond", label = "Request::Validators"]
    av_store    -> chn_api      [arrowhead = "diamond", label = "FinalizedBlockHash"]

    avail_dist  -> net_brdg     [arrowhead = "onormal", label = "Request::SendValidationMessages"]
    avail_dist  -> runt_api     [arrowhead = "diamond", label = "Request::AvailabilityCores"]
    avail_dist  -> net_brdg     [arrowhead = "onormal", label = "ReportPeer"]
    avail_dist  -> av_store     [arrowhead = "diamond", label = "QueryDataAvailability"]
    avail_dist  -> av_store     [arrowhead = "diamond", label = "QueryChunk"]
    avail_dist  -> av_store     [arrowhead = "diamond", label = "StoreChunk"]
    avail_dist  -> runt_api     [arrowhead = "diamond", label = "Request::Validators"]
    avail_dist  -> chn_api      [arrowhead = "diamond", label = "Ancestors"]
    avail_dist  -> runt_api     [arrowhead = "diamond", label = "Request::SessionIndexForChild"]

    avail_rcov  -> net_brdg     [arrowhead = "onormal", label = "ReportPeer"]
    avail_rcov  -> av_store     [arrowhead = "diamond", label = "QueryChunk"]
    avail_rcov  -> net_brdg     [arrowhead = "diamond", label = "ConnectToValidators"]
    avail_rcov  -> net_brdg     [arrowhead = "onormal", label = "SendValidationMessage::Chunk"]
    avail_rcov  -> net_brdg     [arrowhead = "onormal", label = "SendValidationMessage::RequestChunk"]

    bitf_dist   -> net_brdg     [arrowhead = "onormal", label = "ReportPeer"]
    bitf_dist   -> provisioner  [arrowhead = "onormal", label = "ProvisionableData::Bitfield"]
    bitf_dist   -> net_brdg     [arrowhead = "onormal", label = "SendValidationMessage"]
    bitf_dist   -> net_brdg     [arrowhead = "onormal", label = "SendValidationMessage"]
    bitf_dist   -> runt_api     [arrowhead = "diamond", label = "Request::Validatiors"]
    bitf_dist   -> runt_api     [arrowhead = "diamond", label = "Request::SessionIndexForChild"]

    bitf_sign   -> av_store     [arrowhead = "diamond", label = "QueryChunkAvailability"]
    bitf_sign   -> runt_api     [arrowhead = "diamond", label = "Request::AvailabilityCores"]
    bitf_sign   -> bitf_dist    [arrowhead = "onormal", label = "DistributeBitfield"]

    cand_back   -> av_store     [arrowhead = "diamond", label = "StoreAvailableData"]
    cand_back   -> pov_dist     [arrowhead = "diamond", label = "FetchPoV"]
    cand_back   -> cand_val     [arrowhead = "diamond", label = "ValidateFromChainState"]
    cand_back   -> cand_sel     [arrowhead = "onormal", label = "Invalid"]
    cand_back   -> provisioner  [arrowhead = "onormal", label = "ProvisionableData::MisbehaviorReport"]
    cand_back   -> provisioner  [arrowhead = "onormal", label = "ProvisionableData::BackedCandidate"]
    cand_back   -> pov_dist     [arrowhead = "onormal", label = "DistributePoV"]
    cand_back   -> stmt_dist    [arrowhead = "onormal", label = "Share"]

    cand_sel    -> coll_prot    [arrowhead = "diamond", label = "FetchCollation"]
    cand_sel    -> cand_back    [arrowhead = "onormal", label = "Second"]
    cand_sel    -> coll_prot    [arrowhead = "onormal", label = "ReportCollator"]

    cand_val    -> runt_api     [arrowhead = "diamond", label = "Request::PersistedValidationData"]
    cand_val    -> runt_api     [arrowhead = "diamond", label = "Request::ValidationCode"]
    cand_val    -> runt_api     [arrowhead = "diamond", label = "Request::CheckValidationOutputs"]

    coll_gen    -> coll_prot    [arrowhead = "onormal", label = "DistributeCollation"]

    coll_prot   -> net_brdg     [arrowhead = "onormal", label = "ReportPeer"]
    coll_prot   -> net_brdg     [arrowhead = "onormal", label = "Declare"]
    coll_prot   -> net_brdg     [arrowhead = "onormal", label = "AdvertiseCollation"]
    coll_prot   -> net_brdg     [arrowhead = "onormal", label = "Collation"]
    coll_prot   -> net_brdg     [arrowhead = "onormal", label = "RequestCollation"]
    coll_prot   -> cand_sel     [arrowhead = "onormal", label = "Collation"]

    net_brdg    -> avail_dist   [arrowhead = "onormal", label = "NetworkBridgeUpdateV1"]
    net_brdg    -> bitf_dist    [arrowhead = "onormal", label = "NetworkBridgeUpdateV1"]
    net_brdg    -> pov_dist     [arrowhead = "onormal", label = "NetworkBridgeUpdateV1"]
    net_brdg    -> stmt_dist    [arrowhead = "onormal", label = "NetworkBridgeUpdateV1"]
    net_brdg    -> coll_prot    [arrowhead = "onormal", label = "NetworkBridgeUpdateV1"]

    pov_dist    -> net_brdg     [arrowhead = "onormal", label = "SendValidationMessage"]
    pov_dist    -> net_brdg     [arrowhead = "onormal", label = "ReportPeer"]

    provisioner -> cand_back    [arrowhead = "diamond", label = "GetBackedCandidates"]
    provisioner -> chn_api      [arrowhead = "diamond", label = "BlockNumber"]

    stmt_dist   -> net_brdg     [arrowhead = "onormal", label = "SendValidationMessage"]
    stmt_dist   -> net_brdg     [arrowhead = "onormal", label = "ReportPeer"]
    stmt_dist   -> cand_back    [arrowhead = "onormal", label = "Statement"]
    stmt_dist   -> runt_api     [arrowhead = "onormal", label = "Request::Validators"]
    stmt_dist   -> runt_api     [arrowhead = "onormal", label = "Request::SessionIndexForChild"]
}
```

## The Path to Backing

Let's contextualize that diagram a bit by following a parachain block from its creation through finalization.
Parachains can use completely arbitrary processes to generate blocks. The relay chain doesn't know or care about
the details; each parachain just needs to provide a [collator](collators/collation-generation.md).

**Note**: Inter-subsystem communications are relayed via the overseer, but that step is omitted here for brevity.

**Note**: Dashed lines indicate a request/response cycle, where the response is communicated asynchronously via
a oneshot channel. Adjacent dashed lines may be processed in parallel.

```mermaid
sequenceDiagram
    participant Overseer
    participant CollationGeneration
    participant RuntimeApi
    participant CollatorProtocol

    Overseer ->> CollationGeneration: ActiveLeavesUpdate
    loop for each activated head
        CollationGeneration -->> RuntimeApi: Request availability cores
        CollationGeneration -->> RuntimeApi: Request validators

        Note over CollationGeneration: Determine an appropriate ScheduledCore <br/>and OccupiedCoreAssumption

        CollationGeneration -->> RuntimeApi: Request full validation data

        Note over CollationGeneration: Build the collation

        CollationGeneration ->> CollatorProtocol: DistributeCollation
    end
```

The `DistributeCollation` messages that `CollationGeneration` sends to the `CollatorProtocol` contains
two items: a `CandidateReceipt` and `PoV`. The `CollatorProtocol` is then responsible for distributing
that collation to interested validators. However, not all potential collations are of interest. The
`CandidateSelection` subsystem is responsible for determining which collations are interesting, before
`CollatorProtocol` actually fetches the collation.

```mermaid
sequenceDiagram
    participant CollationGeneration
    participant CS as CollatorProtocol::CollatorSide
    participant NB as NetworkBridge
    participant VS as CollatorProtocol::ValidatorSide
    participant CandidateSelection

    CollationGeneration ->> CS: DistributeCollation
    CS -->> NB: ConnectToValidators

    Note over CS,NB: This connects to multiple validators.

    CS ->> NB: Declare
    NB ->> VS: Declare

    Note over CS: Ensure that the connected validator is among<br/>the para's validator set. Otherwise, skip it.

    CS ->> NB: AdvertiseCollation
    NB ->> VS: AdvertiseCollation

    VS ->> CandidateSelection: Collation

    Note over CandidateSelection: Lots of other machinery in play here,<br/>but there are only three outcomes from the<br/>perspective of the `CollatorProtocol`:

    alt happy path
        CandidateSelection -->> VS: FetchCollation
        Activate VS
        VS ->> NB: RequestCollation
        NB ->> CS: RequestCollation
        CS ->> NB: Collation
        NB ->> VS: Collation
        Deactivate VS

    else collation invalid or unexpected
        CandidateSelection ->> VS: ReportCollator
        VS ->> NB: ReportPeer

    else CandidateSelection already selected a different candidate
        Note over CandidateSelection: silently drop
    end
```

Assuming we hit the happy path, flow continues with `CandidateSelection` receiving a `(candidate_receipt, pov)` as
the return value from its
`FetchCollation` request. The only time `CandidateSelection` actively requests a collation is when
it hasn't yet seconded one for some `relay_parent`, and is ready to second.

```mermaid
sequenceDiagram
    participant CS as CandidateSelection
    participant CB as CandidateBacking
    participant CV as CandidateValidation
    participant SD as StatementDistribution
    participant PD as PoVDistribution

    CS ->> CB: Second
    CB -->> CV: ValidateFromChainState

    Note over CB,CV: There's some complication in the source, as<br/>candidates are actually validated in a separate task.

    alt valid
        Note over CB: This is where we transform the CandidateReceipt into a CommittedCandidateReceipt
        CB ->> SD: Create, share SignedStatement
        CB ->> PD: Distribute PoV
    else invalid
        CB ->> CS: Invalid
    end
```
