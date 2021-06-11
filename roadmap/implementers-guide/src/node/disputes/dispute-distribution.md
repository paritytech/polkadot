# Dispute Distribution

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

Protocol: "/polkadot/dispute/1"

Request:

```rust
struct DisputeRequest {
  // Either initiating invalid vote or our own (if we voted invalid).
  invalid_vote: SignedV2<InvalidVote>,
  // Some invalid vote (can be from backing/approval) or our own if we voted
  // valid.
  valid_vote: SignedV2<ValidVote>,
}

struct InvalidVote {
  /// The candidate being disputed.
  candidate_hash: CandidateHash,
  /// The voting validator.
  validator_index: ValidatorIndex,
  /// The session the candidate appears in.
  candidate_session: SessionIndex,
}

struct ValidVote {
  candidate_hash: CandidateHash,
  validator_index: ValidatorIndex,
  candidate_session: SessionIndex,
  kind: ValidDisputeStatementKind,
}
```

Response:

```rust
enum DisputeResponse {
  Confirmed
}
```

#### Vote Recovery

Protocol: "/polkadot/vote-recovery/1"

```rust
struct IHaveVotesRequest {
  candidate_hash: CandidateHash,
  session: SessionIndex,
  votes: VotesBitfield,
}

struct VotesBitfield(pub BitVec<bitvec::order::Lsb0, u8>);
```

Response:

```rust
struct VotesResponse {
  /// All votes we have, but the requester was missing.
  missing: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
  /// Any additional equivocating votes, we transmit those even if the sender
  /// claims to have votes for that validator (as it might only have one).
  equivocating: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
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

A dispute is initiated once a node sends the first `Dispute` wire message,
which must contain an "invalid" vote and some "valid" vote.

The dispute distribution subsystem can instructed to send that message out to
all concerned validators by means of a `DisputeDistributionMessage::SendDispute`
message. That message must contain an invalid vote from the local node and some
valid one, e.g. a backing statement.

We include a valid vote as well so any node regardless of whether it is synced
with the chain or not or has seen backing/approval vote can see that there are
conflicting votes available, hence we have a valid dispute. Nodes will still
need to check whether the disputing votes are somewhat current and not some
stale ones.

### Participating in a Dispute

Upon receiving a `Dispute` message, a dispute distribution will trigger the
import of the received votes via the dispute coordinator
(`DisputeCoordinatorMessage::ImportStatements`). The dispute coordinator will
take care of participating in that dispute if necessary. Once it is done, the
coordinator will send a `DisputeDistributionMessage::SendDispute` message to dispute
distribution. From here, everything is the same as for starting a dispute,
except that if the local node deemed the candidate valid, the `SendDispute`
message will contain a valid vote signed by our node and will contain the
initially received `Invalid` vote.

### Sending of messages

Starting and participting in a dispute are pretty similar from the perspective
of disptute distribution. Once we receive a `SendDispute` message we try to make
sure to get the data out. We keep track of all the parachain validators that
should see the message, which are all the parachain validators of the session
where the dispute happened as they will want to participate in the dispute.  In
addition we also need to get the votes out to all authorities of the current
session (which might be the same or not). Those authorities will not
participtate in the dispute, but need to see the statements so they can include
them in blocks.

We keep track of connected parachain validators and authorities and will issue
warnings in the logs if connected nodes are less than two thirds of the
corresponding sets. We also only consider a message transmitted, once we
received a confirmation message. If not we will keep retrying getting that
message out as long as the dispute is deemed alive. To determine whether a
dispute is still alive we will issue a
`DisputeCoordinatorMessage::ActiveDisputes` message before each retry run. Once
a dispute is no longer live, we will clean up the state coordingly.

To cather with spam issues, we will in a first implementation only consider
disputes of already included data. Therefore only for candidates that are
already available. These are the only disputes representing an actual threat to
the system and are also the easiest to implement with regards to spam.

Votes can still be old/ not relevant. In this case we will drop those messages
and we might want to decrease reputation of peers sending old data.

### Reception

Because we are not forwarding foreign statements, spam is not so much of
an issue. Rate limiting should be implemented at the substrate level, see
[#7750](https://github.com/paritytech/substrate/issues/7750).

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
dispute.

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

The receiver of a `IHaveVotesRequests` message will do the following:

1. See if the sender is missing votes we are aware of - if so, respond with
   those votes. Also send votes of equivocating validators, no matter the
   bitfield.
2. Check whether the sender knows about any votes, we don't know about and if so
   send a `IHaveVotes` request back, with our knowledge.
3. Record the peer's knowledge.

When to send `IHaveVotes` messages:

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

## Disputes for non included candidates

If deemed necessary we can later on also support disputes for non included
candidates, but disputes for those cases have totally different requirements.

First of all such disputes are not time critical. We just want to have
some offender slashed at some point, but we have no risk of finalizing any bad
data.

Second, we won't have availability for such data, but it also really does not
matter as we have relaxed timing requirements as just mentioned. Instead a node
disputing non included candidates, will be responsible for providing the
disputed data initially. Then nodes which did the check already are also
providers of the data, hence distributing load and making prevention of the
dispute from concluding harder and harder over time. Assuming an attacker can
not DoS a node forever, the dispute will succeed eventually, which is all that
matters. And again, even if an attacker managed to prevent such a dispute from
happening somehow, there is no real harm done: There was no serious attack to
begin with.

[DistputeDistributionMessage]: ../../types/overseer-protocol.md#dispute-distribution-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
