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

    net_brdg    -> avail_dist   [arrowhead = "onormal", label = "NetworkBridgeUpdate"]
    net_brdg    -> bitf_dist    [arrowhead = "onormal", label = "NetworkBridgeUpdate"]
    net_brdg    -> pov_dist     [arrowhead = "onormal", label = "NetworkBridgeUpdate"]
    net_brdg    -> stmt_dist    [arrowhead = "onormal", label = "NetworkBridgeUpdate"]
    net_brdg    -> coll_prot    [arrowhead = "onormal", label = "NetworkBridgeUpdate"]

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

## The Path to Inclusion (Node Side)

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
    participant PV as Provisioner
    participant SD as StatementDistribution
    participant PD as PoVDistribution

    CS ->> CB: Second
    % fn validate_and_make_available
    CB -->> CV: ValidateFromChainState

    Note over CB,CV: There's some complication in the source, as<br/>candidates are actually validated in a separate task.

    alt valid
        Note over CB: This is where we transform the CandidateReceipt into a CommittedCandidateReceipt
        % CandidateBackingJob::sign_import_and_distribute_statement
        % CandidateBackingJob::import_statement
        CB ->> PV: ProvisionableData::BackedCandidate
        % CandidateBackingJob::issue_new_misbehaviors
        opt if there is misbehavior to report
            CB ->> PV: ProvisionableData::MisbehaviorReport
        end
        % CandidateBackingJob::distribute_signed_statement
        CB ->> SD: Share
        % CandidateBackingJob::distribute_pov
        CB ->> PD: DistributePoV
    else invalid
        CB ->> CS: Invalid
    end
```

At this point, you'll see that control flows in two directions: to `StatementDistribution` to distribute
the `SignedStatement`, and to `PoVDistribution` to distribute the `PoV`. However, that's largely a mirage:
while the initial implementation distributes `PoV`s by gossip, that's inefficient, and will be replaced
with a system which fetches `PoV`s only when actually necessary.

> TODO: figure out more precisely the current status and plans; write them up

Therefore, we'll follow the `SignedStatement`. The `StatementDistribution` subsystem is largely concerned
with implementing a gossip protocol:

```mermaid
sequenceDiagram
    participant SD as StatementDistribution
    participant NB as NetworkBridge

    alt On receipt of a<br/>SignedStatement from CandidateBacking
        % fn circulate_statement_and_dependents
        SD ->> NB: SendValidationMessage

        Note right of NB: Bridge sends validation message to all appropriate peers
    else On receipt of peer validation message
        NB ->> SD: NetworkBridgeUpdate

        % fn handle_incoming_message
        alt if we aren't already aware of the relay parent for this statement
            SD ->> NB: ReportPeer
        end

        % fn circulate_statement
        opt if we know of peers who haven't seen this message, gossip it
            SD ->> NB: SendValidationMessage
        end
    end
```

But who are these `Listener`s who've asked to be notified about incoming `SignedStatement`s?
Nobody, as yet.

Let's pick back up with the PoV Distribution subsystem.

```mermaid
sequenceDiagram
    participant CB as CandidateBacking
    participant PD as PoVDistribution
    participant Listener
    participant NB as NetworkBridge

    CB ->> PD: DistributePoV

    Note over PD,Listener: Various subsystems can register listeners for when PoVs arrive

    loop for each Listener
        PD ->> Listener: Arc<PoV>
    end

    Note over PD: Gossip to connected peers

    PD ->> NB: SendPoV

    Note over PD,NB: On receipt of a network PoV, PovDistribution forwards it to each Listener.<br/>It also penalizes bad gossipers.
```

Unlike in the case of `StatementDistribution`, there is another subsystem which in various circumstances
already registers a listener to be notified when a new `PoV` arrives: `CandidateBacking`. Note that this
is the second time that `CandidateBacking` has gotten involved. The first instance was from the perspective
of the validator choosing to second a candidate via its `CandidateSelection` subsystem. This time, it's
from the perspective of some other validator, being informed that this foreign `PoV` has been received.

```mermaid
sequenceDiagram
    participant SD as StatementDistribution
    participant CB as CandidateBacking
    participant PD as PoVDistribution
    participant AS as AvailabilityStore

    SD ->> CB: Statement
    % CB::maybe_validate_and_import => CB::kick_off_validation_work
    CB -->> PD: FetchPoV
    Note over CB,PD: This call creates the Listener from the previous diagram

    CB ->> AS: StoreAvailableData
```

At this point, things have gone a bit nonlinear. Let's pick up the thread again with `BitfieldSigning`. As
the `Overseer` activates each relay parent, it starts a `BitfieldSigningJob` which operates on an extremely
simple metric: after creation, it immediately goes to sleep for 1.5 seconds. On waking, it records the state
of the world pertaining to availability at that moment.

```mermaid
sequenceDiagram
    participant OS as Overseer
    participant BS as BitfieldSigning
    participant RA as RuntimeApi
    participant AS as AvailabilityStore
    participant BD as BitfieldDistribution

    OS ->> BS: ActiveLeavesUpdate
    loop for each activated relay parent
        Note over BS: Wait 1.5 seconds
        BS -->> RA: Request::AvailabilityCores
        loop for each availability core
            BS -->> AS: QueryChunkAvailability
        end
        BS ->> BD: DistributeBitfield
    end
```

`BitfieldDistribution` is, like the other `*Distribution` subsystems, primarily interested in implementing
a peer-to-peer gossip network propagating its particular messages. However, it also serves as an essential
relay passing the message along.

```mermaid
sequenceDiagram
    participant BS as BitfieldSigning
    participant BD as BitfieldDistribution
    participant NB as NetworkBridge
    participant PV as Provisioner

    BS ->> BD: DistributeBitfield
    BD ->> PV: ProvisionableData::Bitfield
    BD ->> NB: SendValidationMessage::BitfieldDistribution::Bitfield
```

We've now seen the message flow to the `Provisioner`: both `CandidateBacking` and `BitfieldDistribution`
contribute provisionable data. Now, let's look at that subsystem.

Much like the `BitfieldSigning` subsystem, the `Provisioner` creates a new job for each newly-activated
leaf, and starts a timer. Unlike `BitfieldSigning`, we won't depict that part of the process, because
the `Provisioner` also has other things going on.

```mermaid
sequenceDiagram
    participant A as Arbitrary
    participant PV as Provisioner
    participant CB as CandidateBacking
    participant BD as BitfieldDistribution
    participant RA as RuntimeApi
    participant PI as ParachainsInherentDataProvider

    alt receive provisionable data
        alt
            CB ->> PV: ProvisionableData
        else
            BD ->> PV: ProvisionableData
        end

        loop over stored Senders
            PV ->> A: ProvisionableData
        end

        Note over PV: store bitfields and backed candidates
    else receive request for inherent data
        PI ->> PV: RequestInherentData
        alt we have already constructed the inherent data
            PV ->> PI: send the inherent data
        else we have not yet constructed the inherent data
            Note over PV,PI: Store the return sender without sending immediately
        end
    else timer times out
        note over PV: Waited 2 seconds
        PV -->> RA: RuntimeApiRequest::AvailabilityCores
        Note over PV: construct and store the inherent data
        loop over stored inherent data requests
            PV ->> PI: (SignedAvailabilityBitfields, BackedCandidates)
        end
    end
```

In principle, any arbitrary subsystem could send a `RequestInherentData` to the `Provisioner`. In practice,
only the `ParachainsInherentDataProvider` does so.

The tuple `(SignedAvailabilityBitfields, BackedCandidates, ParentHeader)` is injected by the `ParachainsInherentDataProvider`
into the inherent data. From that point on, control passes from the node to the runtime.
