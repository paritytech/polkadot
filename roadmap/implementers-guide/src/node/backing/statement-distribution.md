# Statement Distribution (Draft)

This is a first draft for the implementer's guide, which I wrote as a dump of
all info and conclusions I could find, but in one place. This helped me a lot to
connect all the "dots", but it may be too much info. I need feedback on the
following:

-   [ ] Where it is too technical, I can move stuff into mod-docs.
-   [ ] And of course, accuracy/thoroughness.

There is some redundancy, but I came across a saying I liked today: "DRY is for
code, not docs." Re-stating the same thing multiple times helped with my
comprehension. But feedback is welcome.

Also, I will convert it to prose once we know what info is staying. The bullets just helped me stay organized.

## Overview

-   ****Goal:**** every well-connected node is aware of every next potential parachain
    block.
-   Validators can either:
    -   receive parachain block from collator, check block, and gossip statement.
    -   receive statements from other validators, check block, gossip forward
        statement if valid.
-   Why:
    -   Validators must have statements from all (?) other validators
    -   Why:
        -   Quorum must be reached before the block author will consider the
            candidate.
        -   We need to store statements from validators who've checked the candidate
            on the relay chain, so we know who to hold accountable in case of
            disputes.
    -   "Validators who aren't assigned to the parachain still listen for the
        attestations because whichever validator ends up being the author of the
        relay-chain block needs to bundle up attested parachain blocks for several
        parachains and place them into the relay-chain block."

### With Async Backing

Async backing changes it so that:

-   We allow inclusion of old parachain candidates validated by current
    validators.
-   We allow inclusion of old parachain candidates validated by old validators.

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

-   The ****prospective parachains**** subsystem maintains prospective "fragment
    trees" which can be used to determine whether a particular parachain candidate
    could possibly be included in the future. Candidates which either are within a
    fragment tree or <span class="underline">would be</span> part of a fragment tree if accepted are said to be
    in the "hypothetical frontier".
-   The ****statement-distribution**** subsystem keeps track of all candidates, and
    updates its knowledge of the hypothetical frontier based on events such as new
    relay parents, new confirmed candidates, and newly backed candidates.
-   We only consider statements as "importable" when the corresponding candidate
    is part of the hypothetical frontier, and only send "importable" statements to
    the backing subsystem itself.

### Cluster Mode

-   Validator nodes are partitioned into groups (with some exceptions), and
    validators within a group at a relay-parent can send each other `Statement`
    messages for any candidates within that group and based on that relay-parent.
    This is referred to as the "cluster" mode.
-   `Seconded` statements must be sent before `Valid` statements.
-   `Seconded` statements may only be sent to other members of the group when the
    candidate is fully known by the validator.
-   Validators in a cluster receiving messages about unknown candidates request
    the candidate (and statements) from other cluster members which have it.
-   Spam considerations
    -   The maximum depth of candidates allowed in asynchronous backing determines the
        maximum amount of `Seconded` statements originating from a validator V which
        each validator in a cluster may send to others. This bounds the number of
        candidates.
    -   There is a small number of validators in each group, which further limits
        the amount of candidates.

&#x2014;

-   We accept candidates which don't fit in the fragment trees of any relay
    parents.
    -   We listen to prospective parachains subsystem to learn of new additions to
        the fragment trees.
    -   Use this to attempt to import the candidate later.

### Grid Mode

-   Every consensus session provides randomness and a fixed validator set, which
    is used to build a redundant grid topology.
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

-   Once the candidate is backed, wait for some time T to pass to collect
    additional statements. Then produce a 'backed candidate packet'
    `(CommittedCandidateReceipt, Statements)`.
-   Members of a backing group produce an announcement of a fully-backed candidate
    (aka "full manifest") when they are finished.
    -   `BackedCandidateManifest`
    -   Manifests are sent along the grid topology to peers who have the relay-parent
        in their implicit view.
    -   Only sent by 1st-hop nodes after downloading the backed candidate packet
        -   TODO: why?
    -   Ignored when received out-of-topology
-   On every local view change, members of the backing group rebroadcast the
    manifest for all candidates under every new relay-parent across the grid.
-   Nodes should send a `BackedCandidateKnown(hash)` (acknowledgement)
    notification to any peer which has sent a manifest, and the candidate has been
    acquired by other means.
    -   This keeps alternative paths through the topology open.
        -   For the purpose of getting additional statements later.
        -   Why: sending ack is another way of completing "manifest exchange" (pre-req
            for receiving a statement, see below)
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
                -   which has sent a `BackedCandidateKnown` (acknowledgement)
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

## Manifest

-   What it is: A message about a known backed candidate, along with a description
    of the statements backing it.
-   Can be one of two kinds:
    -   `Full`
        -   Contains information about the candidate and should be sent to peers who
            may not have the candidate yet.
    -   `Acknowledgement`
        -   Omit information implicit in the candidate, and should be sent to peers
            which are guaranteed to have the candidate already.

### Manifest exchange

-   Occurs between us and a peer in the grid.
-   Why:
    -   Indicates that both nodes know the candidate as valid and backed.
    -   Allows the nodes to send `Statement` messages directly to each other for any
        new statements they might need.
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

## Cluster

-   Direct distribution of unbacked candidates within a group.
-   Why:
    -   because of prospective parachains communication has to happen between
        backing groups on rotation boundaries.
    -   Only have to send unbacked candidates within groups.
-   Bound number of `Seconded` messages per validator per relay-parent.
    -   Why: spam prevention
    -   Validators can try to circumvent this, but they would only consume a few KB
        of memory and it is trivially slashable on chain.
-   `ClusterTracker`
    -   Determine whether to accept/reject messages from other validators in the
        same group
    -   Keep track of what we have sent to other validators in the group and pending
        statements.
-   Protocol: See "Protocol"

## Grid

-   Distribution of backed candidates and late statements outside the group.
-   Protocol: See "Protocol"

### Grid topology

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
    other validator in at most 2 hops.
-   However, we also supplement this with some degree of random propagation.
    -   Why: insert some redundancy, protect against targeted attacks.

### Grid utilities

-   `SessionGridTopology`
    -   Topology representation for a session.
-   `SessionTopologyView`
    -   Our local view of the grid topology for a session.
    -   Built from `SessionGridTopology`, a list of groups and our validator index.
    -   Propagation
        -   For groups that we are in, receive from nobody and send to our X/Y peers.
        -   For groups that we are not part of:
            -   We receive from any validator in the group we share a slice with and
                send to the corresponding X/Y slice in the other dimension.
            -   For any validators we don't share a slice with, we receive from the
                nodes which share a slice with them.
-   `GroupSubView`
    -   Our local view of a subset of the grid topology organized around a specific
        validator group.
-   `GridTracker`
    -   A tracker of knowledge from authorities within the grid for a particular
        relay parent.
-   `GridTracker::import_manifest`
    -   Called when: receiving an incoming manifest (full or partial).
    -   Attempts to import a manifest advertised by a remote peer.
    -   Checks whether the peer is allowed to send us manifests about this group at
        this relay parent.
    -   Does sanity checks on the manifest itself.
    -   On success, returns whether an acknowledgement should be sent in response.
        -   Only occurs when the candidate is already known to be confirmed backed,
            and the validator is in the set we are receiving from.
-   `GridTracker::add_backed_candidate`
    -   Called when: we receive a notification of a candidate being backed
        -   Candidate must have been confirmed first.
        -   Also dispatches backable candidate announcements and acks to the grid
            topology.
    -   Adds a new backed candidate to the grid tracker.
    -   Returns a list of validators we should send to, along with the type of
        manifest to send.
    -   Adds the candidate as confirmed and backed and populates it with previously
        unconfirmed manifests.

### Seconding limit

-   The seconding limit is a per-validator limit.
-   Before asynchronous backing:
    -   We had a rule that every validator was only allowed to second one candidate
        per relay parent.
-   With asynchronous backing:
    -   We have a 'maximum depth' which makes it possible to second multiple
        candidates per relay parent. The seconding limit is set to max depth to set
        an upper bound on candidates entering the system.

## Candidates

-   `Candidates`: a tracker for all known candidates in the view, and whether they
    are confirmed or not.
-   Confirmed:
    -   The first time a validator gets an announcement for an unknown candidate, it
        will send a request for the candidate.
    -   Upon receiving a response and validating it (see
        `UnhandledResponse::validate_response`), will mark the candidate as
        confirmed.
-   TODO: `Candidates::insert_unconfirmed`
-   TODO: `Candidates::confirm_candidate`

## Request Manager

-   Manages pending requests for candidate data, as well as pending responses.
-   See "Request/Response Protocol" for a high-level description of the flow.
-   See module-docs for full details.
-   `RequestManager::next_request`
    -   Yields next request to dispatch, if there is any.
    -   Does some active maintenance of the connected peers.
-   `UnhandledResponse::validate_response`
    -   Valid responses must provide a valid candidate, signatures which match the
        identifier, and enough statements to back the candidate.
    -   Produces a record of misbehavior by peers.
    -   If valid, yields the candidate, PVD, and requested checked statements.

## Glossary

-   ****Acknowledgement:**** A notification that is sent to a validator that already
    has the candidate, to inform them that the sending node knows the candidate.
-   ****Announcement:**** A notification of a backed candidate being known by the
    sending node. Is a full manifest and initiates manifest exchange.
-   ****Manifest:**** A message about a known backed candidate, along with a
    description of the statements backing it.
-   ****Peer:**** Another validator that a validator is connected to.
-   ****Request/response:**** A protocol used to lazily request and receive heavy
    candidate data when needed.
-   ****Reputation:**** Tracks reputation of peers. Applies annoyance cost and good
    behavior benefits.
-   ****Statement:**** Signed statements that can be made about parachain candidates.
    -   ****Seconded:**** Proposal of a parachain candidate. Implicit validity vote.
    -   ****Valid:**** States that a parachain candidate is valid.
-   ****Target:**** Target validator to send a statement to.
-   ****View:**** Current knowledge of the chain state.
    -   ****Explicit view**** / ****immediate view****
        -   The view a peer has of the relay chain heads and highest finalized block.
    -   ****Implicit view****
        -   Derived from the immediate view. Composed of active leaves and minimum
            relay-parents allowed for candidates of various parachains at those leaves.
