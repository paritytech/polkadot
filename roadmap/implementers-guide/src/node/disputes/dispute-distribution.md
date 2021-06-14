# Dispute Distribution

Dispute distribution is responsible for ensuring all concerned validators will be aware of a dispute and have the relevant votes.

## Design Goals

This design should result in a protocol that is:

- resilient to nodes being temporarily unavailable
- make sure nodes are aware of a dispute quickly
- relatively efficient, should not cause too much stress on the network
- be resilient when it comes to spam
- be simple and boring: We want disputes to work when they happen

## Protocol

### Input

[`DisputeDistributionMessage`][DisputeDistributionMessage]

### Output

- [`DisputeCoordinatorMessage::ActiveDisputes`][DisputeParticipationMessage]
- [`DisputeCoordinatorMessage::ImportStatements`][DisputeParticipationMessage]
- [`DisputeCoordinatorMessage::QueryCandidateVotes`][DisputeParticipationMessage]
- [`RuntimeApiMessage`][RuntimeApiMessage]

### Wire format

#### Disputes

Protocol: "/polkadot/send\_dispute/1"

Request:

```rust
struct DisputeRequest {
  // Either initiating invalid vote or our own (if we voted invalid).
  invalid_vote: InvalidVote,
  // Some invalid vote (can be from backing/approval) or our own if we voted
  // valid.
  valid_vote: ValidVote,
}

struct InvalidVote {
  subject: VoteSubject,
  kind: InvalidDisputeStatementKind,
}

struct ValidVote {
  subject: VoteSubject,
  kind: ValidDisputeStatementKind,
}

struct VoteSubject {
  /// The candidate being disputed.
  candidate_hash: CandidateHash,
  /// The voting validator.
  validator_index: ValidatorIndex,
  /// The session the candidate appears in.
  candidate_session: SessionIndex,
  /// The validator signature, that can be verified when constructing a
  /// `SignedDisputeStatement`.
  validator_signature: ValidatorSignature,
}
```

Response:

```rust
enum DisputeResponse {
  Confirmed
}
```

#### Vote Recovery

Protocol: "/polkadot/req\_votes/1"

```rust
struct IHaveVotesRequest {
  candidate_hash: CandidateHash,
  session: SessionIndex,
  valid_votes: Bitfield,
  invalid_votes: Bitfield,
}

```

Response:

```rust
struct VotesResponse {
  /// All votes we have, but the requester was missing.
  missing: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
}
```

## Functionality

Distributing disputes needs to be a reliable protocol. We would like to make as
sure as possible that our vote got properly delivered to all concerned
validators. For this to work, this subsystem won't be gossip based, but instead
will use a request/response protocol for application level confirmations. The
request will be the payload (the actual votes/statements), the response will
be the confirmation. See [above][#wire-format].

### Starting a Dispute

A dispute is initiated once a node sends the first `DisputeRequest` wire message,
which must contain an "invalid" vote and a "valid" vote.

The dispute distribution subsystem can get instructed to send that message out to
all concerned validators by means of a `DisputeDistributionMessage::SendDispute`
message. That message must contain an invalid vote from the local node and some
valid one, e.g. a backing statement.

We include a valid vote as well, so any node regardless of whether it is synced
with the chain or not or has seen backing/approval vote can see that there are
conflicting votes available, hence we have a valid dispute. Nodes will still
need to check whether the disputing votes are somewhat current and not some
stale ones.

### Participating in a Dispute

Upon receiving a `DisputeRequest` message, a dispute distribution will trigger the
import of the received votes via the dispute coordinator
(`DisputeCoordinatorMessage::ImportStatements`). The dispute coordinator will
take care of participating in that dispute if necessary. Once it is done, the
coordinator will send a `DisputeDistributionMessage::SendDispute` message to dispute
distribution. From here, everything is the same as for starting a dispute,
except that if the local node deemed the candidate valid, the `SendDispute`
message will contain a valid vote signed by our node and will contain the
initially received `Invalid` vote.

Note, that we rely on the coordinator to check availability for spam protection
(see below).
In case the current node is only a potential block producer and does not
actually need to recover availability (as it is not going to participate in the
dispute), there is a potential optimization available: The coordinator could
first just check whether we have our piece and only if we don't, try to recover
availability. Our node having a piece would be proof enough of the
data to be available and thus the dispute to not be spam.

### Sending of messages

Starting and participating in a dispute are pretty similar from the perspective
of disptute distribution. Once we receive a `SendDispute` message we try to make
sure to get the data out. We keep track of all the parachain validators that
should see the message, which are all the parachain validators of the session
where the dispute happened as they will want to participate in the dispute.  In
addition we also need to get the votes out to all authorities of the current
session (which might be the same or not and may change during the dispute).
Those authorities will not participate in the dispute, but need to see the
statements so they can include them in blocks.

We keep track of connected parachain validators and authorities and will issue
warnings in the logs if connected nodes are less than two thirds of the
corresponding sets. We also only consider a message transmitted, once we
received a confirmation message. If not, we will keep retrying getting that
message out as long as the dispute is deemed alive. To determine whether a
dispute is still alive we will issue a
`DisputeCoordinatorMessage::ActiveDisputes` message before each retry run. Once
a dispute is no longer live, we will clean up the state accordingly.

### Reception & Spam Considerations

Because we are not forwarding foreign statements, spam is not so much of an
issue as in other subsystems. Rate limiting should be implemented at the
substrate level, see
[#7750](https://github.com/paritytech/substrate/issues/7750). Still we should
make sure that it is not possible via spamming to prevent a dispute concluding
or worse from getting noticed.

Considered attack vectors:

1. Invalid disputes (candidate does not exist) could make us
   run out of resources. E.g. if we recorded every statement, we could run out
   of disk space eventually.
2. An attacker can just flood us with notifications on any notification
   protocol, assuming flood protection is not effective enough, our unbounded
   buffers can fill up and we will run out of memory eventually.
3. Attackers could spam us at a high rate with invalid disputes. Our incoming
   queue of requests could get monopolized by those malicious requests and we
   won't be able to import any valid disputes and we could run out of resources,
   if we tried to process them all in parallel.

For tackling 1, we make sure to not occupy resources before we don't know a
candidate is available. So we will not record statements to disk until we
recovered availability for the candidate or know by some other means that the
dispute is legit.

For 2, we will pick up on any dispute on restart, so assuming that any realistic
memory filling attack will take some time, we should be able to participate in a
dispute under such attacks.

For 3, full monopolization of the incoming queue should not be possible assuming
substrate handles incoming requests in a somewhat fair way. Still we want some
defense mechanisms, at the very least we need to make sure to not exhaust
resources.

The dispute coordinator will notify us
via `DisputeDistributionMessage::ReportCandidateUnavailable` about unavailable
candidates and we can disconnect from such peers/decrease their reputation
drastically. This alone should get us quite far with regards to queue
monopolization, as availability recovery is expected to fail relatively quickly
for unavailable data.

Still if those spam messages come at a very high rate, we might still run out of
resources if we immediately call `DisputeCoordinatorMessage::ImportStatements`
on each one of them. Secondly with our assumption of 1/3 dishonest validators,
getting rid of all of them will take some time, depending on reputation timeouts
some of them might even be able to reconnect eventually.

To mitigate those issues we will process dispute messages with a maximum
parallelism `N`. We initiate import processes for up to `N` candidates in
parallel. Once we reached `N` parallel requests we will start back pressuring on
the incoming requests. This saves us from resource exhaustion.

To reduce impact of malicious nodes further, we can keep track from which nodes the
currently importing statements came from and will drop requests from nodes that
already have imports in flight.

Honest nodes are not expected to send dispute statements at a high rate, but
even if they did:

- we will import at least the first one and if it is valid it will trigger a
  dispute, preventing finality.
- Chances are good that the first sent candidate from a peer is indeed the
  oldest one (if they differ in age at all).
- for the dropped request any honest node will retry sending.
- there will be other nodes notifying us about that dispute as well.
- honest votes have a speed advantage on average. Apart from the very first
  dispute statement for a candidate, which might cause the availability recovery
  process, imports of honest votes will be super fast, while for spam imports
  they will always take some time as we have to wait for availability to fail.

So this general rate limit, that we drop requests from same peers if they come
faster than we can import the statements should not cause any problems for
honest nodes and is in their favour.

Size of `N`: The larger `N` the better we can handle distributed flood attacks
(see previous paragraph), but we also get potentially more availability recovery
processes happening at the same time, which slows down the individual processes.
And we rather want to have one finish quickly than lots slowly at the same time.
On the other hand, valid disputes are expected to be rare, so if we ever exhaust
`N` it is very likely that this is caused by spam and spam recoveries don't cost
too much bandwidth due to empty responses.

Considering that an attacker would need to attack many nodes in parallel to have
any effect, an `N` of 10 seems to be a good compromise. For honest requests, most
of those imports will likely concern the same candidate, and for dishonest ones
we get to disconnect from up to ten colluding adversaries at a time.

For the size of the channel for incoming requests: Due to dropping of repeated
requests from same nodes we can make the channel relatively large without fear
of lots of spam requests sitting there wasting our time, even after we already
blocked a peer. For valid disputes, incoming requests can become bursty. On the
other hand we will also be very quick in processing them. A channel size of 100
requests seems plenty and should be able to handle bursts adequately.

### Node Startup

On startup we need to check with the dispute coordinator for any ongoing
disputes and assume we have not yet sent our statement for those. In case we
find an explicit statement from ourselves via
`DisputeCoordinatorMessage::QueryCandidateVotes` we will pretend to just have
received a `SendDispute` message for that candidate.

## Backing and Approval Votes

Backing and approval votes get imported when they arrive/are created via the
distpute coordinator by corresponding subsystems.

We assume that under normal operation each node will be aware of backing and
approval votes and optimize for that case. Nevertheless we want disputes to
conclude fast and reliable, therefore if a node is not aware of backing/approval
votes it can request the missing votes from the node that informed it about the
dispute (see [Resiliency](#Resiliency])

## Resiliency

The above protocol should be sufficient for most cases, but there are certain
cases we also want to have covered:

- Non validator nodes might be interested in ongoing voting, even before it is
  recorded on chain.
- Nodes might have missed votes, especially backing or approval votes.
  Recovering them from chain is difficult and expensive, due to runtime upgrades
  and untyped extrinsics.

To cover those cases, we introduce a second request/response protocol, which can
be handled on a lower priority basis as the one above. It consists of the
request/response messages as described in the [protocol
section][#vote-recovery].

Nodes may send those requests to validators, if they feel they are missing
votes. E.g. after some timeout, if no majority was reached yet in their point of
view or if they are not aware of any backing/approval votes for a received
disputed candidate.

The receiver of a `IHaveVotesRequest` message will do the following:

1. See if the sender is missing votes we are aware of - if so, respond with
   those votes.
2. Check whether the sender knows about any votes, we don't know about and if so
   send a `IHaveVotesRequest` request back, with our knowledge.
3. Record the peer's knowledge.

When to send `IHaveVotesRequest` messages:

1. Whenever we are asked to do so via
   `DisputeDistributionMessage::FetchMissingVotes`.
2. Approximately once per block to some random validator as long as the dispute
   is active.

Spam considerations: Nodes want to accept those messages once per validator and
per slot. They are free to drop more frequent requests or requests for stale
data. Requests coming from non validator nodes, can be handled on a best effort
basis.

## Considerations

Dispute distribution is critical. We should keep track of available validator
connections and issue warnings if we are not connected to a majority of
validators. We should also keep track of failed sending attempts and log
warnings accordingly. As disputes are rare and TCP is a reliable protocol,
probably each failed attempt should trigger a warning in logs and also logged
into some Prometheus metric.

## Disputes for non available candidates

If deemed necessary we can later on also support disputes for non available
candidates, but disputes for those cases have totally different requirements.

First of all such disputes are not time critical. We just want to have
some offender slashed at some point, but we have no risk of finalizing any bad
data.

Second, as we won't have availability for such data, the node that initiated the
dispute will be responsible for providing the disputed data initially. Then
nodes which did the check already are also providers of the data, hence
distributing load and making prevention of the dispute from concluding harder
and harder over time. Assuming an attacker can not DoS a node forever, the
dispute will succeed eventually, which is all that matters. And again, even if
an attacker managed to prevent such a dispute from happening somehow, there is
no real harm done: There was no serious attack to begin with.

[DistputeDistributionMessage]: ../../types/overseer-protocol.md#dispute-distribution-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
