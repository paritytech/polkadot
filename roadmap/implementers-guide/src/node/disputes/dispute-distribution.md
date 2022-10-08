# Dispute Distribution

Dispute distribution is responsible for ensuring all concerned validators will
be aware of a dispute and have the relevant votes.

## Design Goals

This design should result in a protocol that is:

- resilient to nodes being temporarily unavailable
- make sure nodes are aware of a dispute quickly
- relatively efficient, should not cause too much stress on the network
- be resilient when it comes to spam
- be simple and boring: We want disputes to work when they happen

## Protocol

Distributing disputes needs to be a reliable protocol. We would like to make as
sure as possible that our vote got properly delivered to all concerned
validators. For this to work, this subsystem won't be gossip based, but instead
will use a request/response protocol for application level confirmations. The
request will be the payload (the actual votes/statements), the response will
be the confirmation. See [below][#wire-format].

### Input

[`DisputeDistributionMessage`][DisputeDistributionMessage]

### Output

- [`DisputeCoordinatorMessage::ActiveDisputes`][DisputeCoordinatorMessage]
- [`DisputeCoordinatorMessage::ImportStatements`][DisputeCoordinatorMessage]
- [`DisputeCoordinatorMessage::QueryCandidateVotes`][DisputeCoordinatorMessage]
- [`RuntimeApiMessage`][RuntimeApiMessage]

### Wire format

#### Disputes

Protocol: `"/<genesis_hash>/<fork_id>/send_dispute/1"`

Request:

```rust
struct DisputeRequest {
  /// The candidate being disputed.
  pub candidate_receipt: CandidateReceipt,

  /// The session the candidate appears in.
  pub session_index: SessionIndex,

  /// The invalid vote data that makes up this dispute.
  pub invalid_vote: InvalidDisputeVote,

  /// The valid vote that makes this dispute request valid.
  pub valid_vote: ValidDisputeVote,
}

/// Any invalid vote (currently only explicit).
pub struct InvalidDisputeVote {
  /// The voting validator index.
  pub validator_index: ValidatorIndex,

  /// The validator signature, that can be verified when constructing a
  /// `SignedDisputeStatement`.
  pub signature: ValidatorSignature,

  /// Kind of dispute statement.
  pub kind: InvalidDisputeStatementKind,
}

/// Any valid vote (backing, approval, explicit).
pub struct ValidDisputeVote {
  /// The voting validator index.
  pub validator_index: ValidatorIndex,

  /// The validator signature, that can be verified when constructing a
  /// `SignedDisputeStatement`.
  pub signature: ValidatorSignature,

  /// Kind of dispute statement.
  pub kind: ValidDisputeStatementKind,
}
```

Response:

```rust
enum DisputeResponse {
  Confirmed
}
```

#### Vote Recovery

Protocol: `"/<genesis_hash>/<fork_id>/req_votes/1"`

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

## Starting a Dispute

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

## Participating in a Dispute

Upon receiving a `DisputeRequest` message, a dispute distribution will trigger the
import of the received votes via the dispute coordinator
(`DisputeCoordinatorMessage::ImportStatements`). The dispute coordinator will
take care of participating in that dispute if necessary. Once it is done, the
coordinator will send a `DisputeDistributionMessage::SendDispute` message to dispute
distribution. From here, everything is the same as for starting a dispute,
except that if the local node deemed the candidate valid, the `SendDispute`
message will contain a valid vote signed by our node and will contain the
initially received `Invalid` vote.

Note, that we rely on `dispute-coordinator` to check validity of a dispute for spam
protection (see below).

## Sending of messages

Starting and participating in a dispute are pretty similar from the perspective
of dispute distribution. Once we receive a `SendDispute` message, we try to make
sure to get the data out. We keep track of all the parachain validators that
should see the message, which are all the parachain validators of the session
where the dispute happened as they will want to participate in the dispute.  In
addition we also need to get the votes out to all authorities of the current
session (which might be the same or not and may change during the dispute).
Those authorities will not participate in the dispute, but need to see the
statements so they can include them in blocks.

### Reliability

We only consider a message transmitted, once we received a confirmation message.
If not, we will keep retrying getting that message out as long as the dispute is
deemed alive. To determine whether a dispute is still alive we will ask the
`dispute-coordinator` for a list of all still active disputes via a
`DisputeCoordinatorMessage::ActiveDisputes` message before each retry run. Once
a dispute is no longer live, we will clean up the state accordingly.

### Order

We assume `SendDispute` messages are coming in an order of importance, hence
`dispute-distribution` will make sure to send out network messages in the same
order, even on retry.

### Rate Limit

For spam protection (see below), we employ an artificial rate limiting on sending
out messages in order to not hit the rate limit at the receiving side, which
would result in our messages getting dropped and our reputation getting reduced.

## Reception

As we shall see the receiving side is mostly about handling spam and ensuring
the dispute-coordinator learns about disputes as fast as possible.

Goals for the receiving side:

1. Get new disputes to the dispute-coordinator as fast as possible, so
  prioritization can happen properly.
2. Batch votes per disputes as much as possible for good import performance.
3. Prevent malicious nodes exhausting node resources by sending lots of messages.
4. Prevent malicious nodes from sending so many messages/(fake) disputes,
  preventing us from concluding good ones.
5. Limit ability of malicious nodes of delaying the vote import due to batching
   logic.

Goal 1 and 2 seem to be conflicting, but an easy compromise is possible: When
learning about a new dispute, we will import the vote immediately, making the
dispute coordinator aware and also getting immediate feedback on the validity.
Then if valid we can batch further incoming votes, with less time constraints as
the dispute-coordinator already knows about the dispute.

Goal 3 and 4 are obviously very related and both can easily be solved via rate
limiting as we shall see below. Rate limits should already be implemented at the
substrate level, but [are not](https://github.com/paritytech/substrate/issues/7750)
at the time of writing. But even if they were, the enforced substrate limits would
likely not be configurable and thus would still be to high for our needs as we can
rely on the following observations:

1. Each honest validator will only send one message (apart from duplicates on
  timeout) per candidate/dispute.
2. An honest validator needs to fully recover availability and validate the
  candidate for casting a vote.

With these two observations, we can conclude that honest validators will usually
not send messages at a high rate. We can therefore enforce conservative rate
limits and thus minimize harm spamming malicious nodes can have.

Before we dive into how rate limiting solves all spam issues elegantly, let's
discuss that honest behaviour further:

What about session changes? Here we might have to inform a new validator set of
lots of already existing disputes at once.

With observation 1) and a rate limit that is per peer, we are still good:

Let's assume a rate limit of one message per 200ms per sender. This means 5
messages from each validator per second. 5 messages means 5 disputes!
Conclusively, we will be able to conclude 5 disputes per second - no matter what
malicious actors are doing. This is assuming dispute messages are sent ordered,
but even if not perfectly ordered: On average it will be 5 disputes per second.

This is good enough! All those disputes are valid ones and will result in
slashing and disabling of validators. Let's assume all of them conclude `valid`,
and we disable validators only after 100 raised concluding valid disputes, we
would still start disabling misbehaving validators in only 20 seconds.

One could also think that in addition participation is expected to take longer,
which means on average we can import/conclude disputes faster than they are
generated - regardless of dispute spam. Unfortunately this is not necessarily
true: There might be parachains with very light load where recovery and
validation can be accomplished very quickly - maybe faster than we can import
those disputes.

This is probably an argument for not imposing a too low rate limit, although the
issue is more general: Even without any rate limit, if an attacker generates
disputes at a very high rate, nodes will be having trouble keeping participation
up, hence the problem should be mitigated at a [more fundamental
layer](https://github.com/paritytech/polkadot/issues/5898).

For nodes that have been offline for a while, the same argument as for session
changes holds, but matters even less: We assume 2/3 of nodes to be online, so
even if the worst case 1/3 offline happens and they could not import votes fast
enough (as argued above, they in fact can) it would not matter for consensus.

### Rate Limiting

As suggested previously, rate limiting allows to mitigate all threats that come
from malicious actors trying to overwhelm the system in order to get away without
a slash, when it comes to dispute-distribution. In this section we will explain
how in greater detail.

The idea is to open a queue with limited size for each peer. We will process
incoming messages as fast as we can by doing the following:

1. Check that the sending peer is actually a valid authority - otherwise drop
   message and decrease reputation/disconnect.
2. Put message on the peer's queue, if queue is full - drop it.

Every `RATE_LIMIT` seconds (or rather milliseconds), we pause processing
incoming requests to go a full circle and process one message from each queue.
Processing means `Batching` as explained in the next section.

### Batching

To achieve goal 2 we will batch incoming votes/messages together before passing
them on as a single batch to the `dispute-coordinator`. To adhere to goal 1 as
well, we will do the following:

1. For an incoming message, we check whether we have an existing batch for that
   candidate, if not we import directly to the dispute-coordinator, as we have
   to assume this is concerning a new dispute.
2. We open a batch and start collecting incoming messages for that candidate,
   instead of immediately forwarding.
4. We keep collecting votes in the batch until we receive less than
   `MIN_KEEP_BATCH_ALIVE_VOTES` unique votes in the last `BATCH_COLLECTING_INTERVAL`. This is
   important to accommodate for goal 5 and also 3.
5. We send the whole batch to the dispute-coordinator.

This together with rate limiting explained above ensures we will be able to
process valid disputes: We can limit the number of simultaneous existing batches
to some high value, but can be rather certain that this limit will never be
reached - hence we won't drop valid disputes:

Let's assume `MIN_KEEP_BATCH_ALIVE_VOTES` is 10, `BATCH_COLLECTING_INTERVAL`
is `500ms` and above `RATE_LIMIT` is `100ms`. 1/3 of validators are malicious,
so for 1000 this means around 330 malicious actors worst case.

All those actors can send a message every `100ms`, that is 10 per second. This
means at the begining of an attack they can open up around 3300 batches. Each
containing two votes. So memory usage is still negligible. In reality it is even
less, as we also demand 10 new votes to trickle in per batch in order to keep it
alive, every `500ms`. Hence for the first second, each batch requires 20 votes
each. Each message is 2 votes, so this means 10 messages per batch. Hence to
keep those batches alive 10 attackers are needed for each batch. This reduces
the number of opened batches by a factor of 10: So we only have 330 batches in 1
second - each containing 20 votes.

The next second: In order to further grow memory usage, attackers have to
maintain 10 messages per batch and second. Number of batches equals the number
of attackers, each has 10 messages per second, all are needed to maintain the
batches in memory. Therefore we have a hard cap of around 330 (number of
malicious nodes) open batches. Each can be filled with number of malicious
actor's votes. So 330 batches with each 330 votes: Let's assume approximately 100
bytes per signature/vote. This results in a worst case memory usage of 330 * 330
* 100 ~= 10 MiB.

For 10_000 validators, we are already in the Gigabyte range, which means that
with a validator set that large we might want to be more strict with the rate limit or
require a larger rate of incoming votes per batch to keep them alive.

For a thousand validators a limit on batches of around 1000 should never be
reached in practice. Hence due to rate limiting we have a very good chance to
not ever having to drop a potential valid dispute due to some resource limit.

Further safe guards are possible: The dispute-coordinator actually
confirms/denies imports. So once we receive a denial by the dispute-coordinator
for the initial imported votes, we can opt into flushing the batch immediately
and importing the votes. This swaps memory usage for more CPU usage, but if that
import is deemed invalid again we can immediately decrease the reputation of the
sending peers, so this should be a net win. For the time being we punt on this
for simplicity.

Instead of filling batches to maximize memory usage, attackers could also try to
overwhelm the dispute coordinator by only sending votes for new candidates all
the time. This attack vector is mitigated also by above rate limit and
decreasing the peer's reputation on denial of the invalid imports by the
coordinator.

### Node Startup

On startup we need to check with the dispute coordinator for any ongoing
disputes and assume we have not yet sent our statement for those. In case we
find an explicit statement from ourselves via
`DisputeCoordinatorMessage::QueryCandidateVotes` we will pretend to just have
received a `SendDispute` message for that candidate.

## Backing and Approval Votes

Backing and approval votes get imported when they arrive/are created via the
dispute coordinator by corresponding subsystems.

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
- More importantly, on era changes the new authority set, from the perspective
  of approval-voting have no need to see "old" approval votes, hence they might
  not see them, can therefore not import them into the dispute coordinator and
  therefore no authority will put them on chain.

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

[DisputeDistributionMessage]: ../../types/overseer-protocol.md#dispute-distribution-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
