# Statement Distribution (Draft)

## Overview

-   **Goal:** every well-connected node is aware of every next potential parachain
    block.
-   Validators can either:
    -   receive parachain block from collator, check block, and gossip statement.
    -   receive statements from other validators, check the parachain block if
        it originated within their own group, gossip forward statement if valid.
-   Why:
    -   Validators must have statements, candidates, and persisted validation
        from all other validators.
    -   Why:
        -   We need to store statements from validators who've checked the candidate
            on the relay chain, so we know who to hold accountable in case of
            disputes.
        -   Any validator can be selected as the next relay-chain block author,
            and this is not revealed in advance for security reasons.
        -   As a result, all validators must have a up to date view of all
            possible parachain candidates + backing statements that could be
            placed on-chain in the next block.
        -   Backing-group quorum (that is, enough backing group votes) must be
            reached before the block author will consider the candidate.
    -   Validators need to consider _all_ seconded candidates within their own
        group, because that's what they're assigned to work on. Validators only
        need to consider backable candidates from other groups.
    -   "Validators who aren't assigned to the parachain still listen for the
        attestations because whichever validator ends up being the author of the
        relay-chain block needs to bundle up attested parachain blocks for several
        parachains and place them into the relay-chain block."

### With Async Backing

Asynchronous backing changes the runtime to accept parachain candidates from a
certain allowed range of historic relay-parents. These candidates must be backed
by the group assigned to the parachain as-of their corresponding relay parent.

## Protocol

To address the concern of dealing with large numbers of spam candidates or
statements, the overall design approach is to combine a focused "clustering"
protocol for legitimate fresh candidates with a broad-distribution "grid"
protocol to quickly get backed candidates into the hands of many validators.
Validators do not eagerly send each other heavy `CommittedCandidateReceipt`,
but instead request these lazily through request/response protocols.

A high-level description of the protocol follows:

### Messages

Nodes can send each other a few kinds of messages: `Statement`,
`BackedCandidateManifest`, `BackedCandidateAcknowledgement`.

-   `Statement` messages contain only a signed compact statement, without full
    candidate info.
-   `BackedCandidateManifest` messages advertise a description of a backed
    candidate and stored statements.
-   `BackedCandidateAcknowledgement` messages acknowledge that a backed candidate
    is fully known.

### Request/response protocol

Nodes can request the full `CommittedCandidateReceipt` and
`PersistedValidationData`, along with statements, over a request/response
protocol. This is the `AttestedCandidateRequest`; the response is
`AttestedCandidateResponse`.

### Importability and the Hypothetical Frontier

-   The **prospective parachains** subsystem maintains prospective "fragment
    trees" which can be used to determine whether a particular parachain candidate
    could possibly be included in the future. Candidates which either are within a
    fragment tree or <span class="underline">would be</span> part of a fragment tree if accepted are said to be
    in the "hypothetical frontier".
-   The **statement-distribution** subsystem keeps track of all candidates, and
    updates its knowledge of the hypothetical frontier based on events such as new
    relay parents, new confirmed candidates, and newly backed candidates.
-   We only consider statements as "importable" when the corresponding candidate
    is part of the hypothetical frontier, and only send "importable" statements to
    the backing subsystem itself.

### Cluster Mode

-   Validator nodes are partitioned into groups (with some exceptions), and
    validators within a group at a relay-parent can send each other `Statement`
    messages for any candidates within that group and based on that relay-parent.
-   This is referred to as the "cluster" mode. Clusters are the same as backing
    groups.
-   `Seconded` statements must be sent before `Valid` statements.
-   `Seconded` statements may only be sent to other members of the group when the
    candidate is fully known by the local validator.
    -   "Fully known" means the validator has the full
        `CommittedCandidateReceipt` and `PersistedValidationData`, which it
        receives on request from other validators or from a collator.
    -   The reason for this is that sending a statement (which is always a
        `CompactStatement` carrying nothing but a hash and signature) to the
        cluster, is also signal that you are available to request the candidate
        from.
    -   This makes the protocol easier to reason about, while also reducing
        network messages about candidates that don't really exist.
-   Validators in a cluster receiving messages about unknown candidates request
    the candidate (and statements) from other cluster members which have it.
-   Spam considerations
    -   The maximum depth of candidates allowed in asynchronous backing determines the
        maximum amount of `Seconded` statements originating from a validator V which
        each validator in a cluster may send to others. This bounds the number of
        candidates.
    -   There is a small number of validators in each group, which further limits
        the amount of candidates.
-   We accept candidates which don't fit in the fragment trees of any relay
    parents.
    -   We listen to prospective parachains subsystem to learn of new additions to
        the fragment trees.
    -   Use this to attempt to import the candidate later.

### Grid Mode

-   Every consensus session provides randomness and a fixed validator set, which
    is used to build a redundant grid topology.
    -    It's redundant in the sense that there are 2 paths from every node to
         every other node.
-   This grid topology is used to create "sending" and "receiving" paths from each
    validator group to every validator.
-   When a node observes a candidate as backed, it sends a
    `BackedCandidateManifest` to their "receiving" nodes.
-   If receiving nodes don't yet know the candidate, they request it.
-   Once they know the candidate, they respond with a
    `BackedCandidateAcknowledgement`.
-   Once two nodes perform a manifest/acknowledgement exchange, they can send
    `Statement` messages directly to each other for any new statements they might
    need.
    -   This limits the amount of statements we'd have to deal with w.r.t.
        candidates that don't really exist. See "Manifest Exchange" section.
-   There are limitations on the number of candidates that can be advertised by
    each peer, similar to those in the cluster. Validators do not request
    candidates which exceed these limitations.
-   Validators request candidates as soon as they are advertised, but do not
    import the statements until the candidate is part of the hypothetical
    frontier, and do not re-advertise or acknowledge until the candidate is
    considered both backed and part of the hypothetical frontier.
-   Note that requesting is not an implicit acknowledgement, and an explicit
    acknowledgement must be sent upon receipt.

### Old Grid Protocol

-   Once the candidate is backed, produce a 'backed candidate packet'
    `(CommittedCandidateReceipt, Statements)`.
-   Members of a backing group produce an announcement of a fully-backed candidate
    (aka "full manifest") when they are finished.
    -   `BackedCandidateManifest`
    -   Manifests are sent along the grid topology to peers who have the relay-parent
        in their implicit view.
    -   Only sent by 1st-hop nodes after downloading the backed candidate packet.
        -   The grid topology is a 2-dimensional grid that provides either a 1
            or 2-hop path from any originator to any recipient - 1st-hop nodes
            are those which share either a row or column with the originator,
            and 2nd-hop nodes are those which share a column or row with that
            1st-hop node.
        -   Note that for the purposes of statement distribution, we actually
            take the union of the routing paths from each validator in a group
            to the local node to determine the sending and receiving paths.
    -   Ignored when received out-of-topology
-   On every local view change, members of the backing group rebroadcast the
    manifest for all candidates under every new relay-parent across the grid.
-   Nodes should send a `BackedCandidateAcknowledgement(CandidateHash,
    StatementFilter)` notification to any peer which has sent a manifest, and
    the candidate has been acquired by other means.
-   Request/response for the candidate + votes.
    -   Ignore if they are inconsistent with the manifest.
    -   A malicious backing group is capable of producing an unbounded number of
        backed candidates.
        -   We request the candidate only if the candidate has a hypothetical depth in
            any of our fragment trees, and:
        -   the seconding validators have not seconded any other candidates at that
            depth in any of those fragment trees
-   All members of the group attempt to circulate all statements (in compact form)
    from the rest of the group on candidates that have already been backed.
    -   They do this via the grid topology.
    -   They add the statements to their backed candidate packet for future
        requestors, and also:
        -   send the statement to any peer, which:
            -   we advertised the backed candidate to (sent manifest), and:
                -   has previously & successfully requested the backed candidate packet,
                    or:
                -   which has sent a `BackedCandidateAcknowledgement`
    -   1st-hop nodes do the same thing

## Statement distribution messages

### Input

-   `ActiveLeavesUpdate`
    -   Notification of a change in the set of active leaves.
-   `StatementDistributionMessage::Share`
    -   Notification of a locally-originating statement.
    -   Handled by `share_local_statement`
-   `StatementDistributionMessage::Backed`
    -   Notification of a candidate being backed (received enough validity votes
        from the backing group).
    -   Handled by `handle_backed_candidate_message`
-   `StatementDistributionMessage::NetworkBridgeUpdate`
    -   Handled by `handle_network_update`
    -   v1 compatibility
    -   `Statement`
        -   Notification of a signed statement.
        -   Handled by `handle_incoming_statement`
    -   `BackedCandidateManifest`
        -   Notification of a backed candidate being known by the sending node.
        -   For the candidate being requested by the receiving node if needed.
        -   Announcement
        -   Handled by `handle_incoming_manifest`
    -   `BackedCandidateKnown`
        -   Notification of a backed candidate being known by the sending node.
        -   For informing a receiving node which already has the candidate.
        -   Acknowledgement.
        -   Handled by `handle_incoming_acknowledgement`

### Output

-   `NetworkBridgeTxMessage::SendValidationMessages`
    -   Sends a peer all pending messages / acknowledgements / statements for a
        relay parent, either through the cluster or the grid.
-   `NetworkBridgeTxMessage::SendValidationMessage`
    -   Circulates a compact statement to all peers who need it, either through the
        cluster or the grid.
-   `NetworkBridgeTxMessage::ReportPeer`
    -   Reports a peer (either good or bad).
-   `CandidateBackingMessage::Statement`
    -   Note a validator's statement about a particular candidate.
-   `ProspectiveParachainsMessage::GetHypotheticalFrontier`
    -   Gets the hypothetical frontier membership of candidates under active leaves'
        fragment trees.
-   `NetworkBridgeTxMessage::SendRequests`
    -   Sends requests, initiating request/response protocol.

## Request/Response

-   Why:
    -   Validators do not eagerly send each other heavy
        `CommittedCandidateReceipt`, but instead request these lazily through
        request/response protocols.

### Protocol

1.  Requesting Validator

    -   Requests are queued up with `RequestManager::get_or_insert`.
        -   Done as needed, when handling incoming manifests/statements.
    -   `RequestManager::dispatch_requests` sends any queued-up requests.
        -   Calls `RequestManager::next_request` to completion.
            -   Creates the `OutgoingRequest`, saves the receiver in
                `RequestManager::pending_responses`.
        -   Does nothing if we have more responses pending than the limit of parallel
            requests.

2.  Peer

    -   Requests come in on a peer on the `IncomingRequestReceiver`.
        -   Runs in a background responder task which feeds requests to `answer_request`
            through `MuxedMessage`.
        -   This responder task has a limit on the number of parallel requests.
    -   `answer_request` on the peer takes the request and sends a response.
        -   Does this using the response sender on the request.

3.  Requesting Validator

    -   `receive_response` on the original validator yields a response.
        -   Response was sent on the request's response sender.
        -   Uses `RequestManager::await_incoming` to await on pending responses in an
            unordered fashion.
        -   Runs on the `MuxedMessage` receiver.
    -   `handle_response` handles the response.

### API

-   `dispatch_requests`
    -   Dispatches pending requests for candidate data & statements.
-   `answer_request`
    -   Answers an incoming request for a candidate.
    -   Takes an incoming `AttestedCandidateRequest`.
-   `receive_response`
    -   Wait on the next incoming response.
    -   If there are no requests pending, this future never resolves.
    -   Returns `UnhandledResponse`
-   `handle_response`
    -   Handles an incoming response.
    -   Takes `UnhandledResponse`

## Manifests

-   What it is: A message about a known backed candidate, along with a description
    of the statements backing it.
-   Can be one of two kinds:
    -   `Full`
        -   Contains information about the candidate and should be sent to peers who
            may not have the candidate yet.
    -   `Acknowledgement`
        -   Omit information implicit in the candidate, and should be sent to peers
            which are guaranteed to have the candidate already.

### Manifest Exchange

-   Occurs between us and a peer in the grid.
-   Indicates that both nodes know the candidate as valid and backed.
-   Allows the nodes to send `Statement` messages directly to each other for any
    new statements.
-   Why:
    -   This limits the amount of statements we'd have to deal with w.r.t.
        candidates that don't really exist.
    -   Limiting out-of-group statement distribution between peers to only
        candidates that both peers agree are backed and exist, ensures we only
        have to store statements about real candidates.
-   Both `manifest_sent_to` and `manifest_received_from` have been invoked.
-   In practice, it means that one of three things have happened:
    -   They announced, we acknowledged
    -   We announced, they acknowledged
    -   We announced, they announced (not sure if this can actually happen; it would
        happen if 2 nodes had each other in their sending set and they sent
        manifests at the same time. The code accounts for this anyway)
-   After conclusion, we update pending statements.
    -   We now know our statements and theirs.
    -   Pending statements are those we know locally that the remote node does not.

#### Alternative Paths Through The Topology

-   Nodes should send a `BackedCandidateAcknowledgement(CandidateHash,
    StatementFilter)` notification to any peer which has sent a manifest, and
    the candidate has been acquired by other means.
    -   This keeps alternative paths through the topology open.
        -   For the purpose of getting additional statements that come later,
            but not after the candidate has been posted on-chain.
        -   This is mostly about the limitation that the runtime has no way for
            block authors to post statements that come after the parablock is
            posted on-chain and ensure those validators still get rewarded.
        -   Technically, we only need enough statements to back the candidate
            and the manifest + request will provide that. But more statements
            might come shortly afterwards, and we want those to end up on-chain
            as well to ensure all validators in the group are rewarded.
        -   For clarity, here is the full timeline:

            -   candidate seconded
            -   backable in cluster
            -   distributed along grid
            -   latecomers issue statements
            -   candidate posted on chain
            -   really latecomers issue statements

## Cluster Module

-   Direct distribution of unbacked candidates within a group.
-   Why:
    -   because of prospective parachains communication has to happen between
        backing groups on rotation boundaries.
    -   Only have to send unbacked candidates within groups.
-   Bound number of `Seconded` messages per validator per relay-parent.
    -   Why: spam prevention
    -   Validators can try to circumvent this, but they would only consume a few KB
        of memory and it is trivially slashable on chain.
-   Determines whether to accept/reject messages from other validators in the
    same group.
-   Keeps track of what we have sent to other validators in the group, and
    pending statements.
-   Protocol: See "Protocol"

## Grid Module

-   Distribution of backed candidates and late statements outside the group.
-   Protocol: See "Protocol"

### Grid Topology

-   The gossip topology
    -   Why: limit the amount of peers we send messages to and handle view updates.
-   The basic operation of the 2D grid topology is that:
    -   A validator producing a message sends it to its row-neighbors and its
        column-neighbors
    -   A validator receiving a message originating from one of its row-neighbors
        sends it to its column-neighbors
    -   A validator receiving a message originating from one of its column-neighbors
        sends it to its row-neighbors
-   This grid approach defines 2 unique paths for every validator to reach every
    other validator in at most 2 hops, providing redundancy.
-   Propagation
    -   For groups that we are in, receive from nobody and send to our X/Y peers.
    -   For groups that we are not part of:
        -   We receive from any validator in the group we share a slice with and
            send to the corresponding X/Y slice in the other dimension.
        -   For any validators we don't share a slice with, we receive from the
            nodes which share a slice with them.

### Seconding Limit

-   The seconding limit is a per-validator limit.
-   Before asynchronous backing:
    -   We had a rule that every validator was only allowed to second one candidate
        per relay parent.
-   With asynchronous backing:
    -   We have a 'maximum depth' which makes it possible to second multiple
        candidates per relay parent. The seconding limit is set to ~max depth +
        `~ to set an upper bound on candidates entering the system.

## Candidates Module

-   Provides a tracker for all known candidates in the view, and whether they
    are confirmed or not.
-   Confirmed:
    -   The first time a validator gets an announcement for an unknown candidate, it
        will send a request for the candidate.
    -   Upon receiving a response and validating it (see
        `UnhandledResponse::validate_response`), will mark the candidate as
        confirmed.

## Requests Module

-   Provides a manager for pending requests for candidate data, as well as
    pending responses.
-   See "Request/Response Protocol" for a high-level description of the flow.
-   See module-docs for full details.

## Glossary

-   **Acknowledgement:** A notification that is sent to a validator that already
    has the candidate, to inform them that the sending node knows the candidate.
-   **Announcement:** A notification of a backed candidate being known by the
-   **Attestation:** See "Statement".
    sending node. Is a full manifest and initiates manifest exchange.
-   **Manifest:** A message about a known backed candidate, along with a
    description of the statements backing it. See "Manifests" section.
-   **Peer:** Another validator that a validator is connected to.
-   **Request/response:** A protocol used to lazily request and receive heavy
    candidate data when needed.
-   **Reputation:** Tracks reputation of peers. Applies annoyance cost and good
    behavior benefits.
-   **Statement:** Signed statements that can be made about parachain candidates.
    -   **Seconded:** Proposal of a parachain candidate. Implicit validity vote.
    -   **Valid:** States that a parachain candidate is valid.
-   **Target:** Target validator to send a statement to.
-   **View:** Current knowledge of the chain state.
    -   **Explicit view** / **immediate view**
        -   The view a peer has of the relay chain heads and highest finalized block.
    -   **Implicit view**
        -   Derived from the immediate view. Composed of active leaves and minimum
            relay-parents allowed for candidates of various parachains at those leaves.
