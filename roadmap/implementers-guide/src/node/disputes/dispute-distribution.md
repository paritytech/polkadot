# Dispute Distribution

## Protocol

### Input

[`DisputeDistributionMessage`][DisputeDistributionMessage]

### Output

- [`DisputeCoordinatorMessage::ActiveDisputes`][DisputeParticipationMessage]
- [`DisputeCoordinatorMessage::ImportStatements`][DisputeParticipationMessage]
- [`RuntimeApiMessage`][RuntimeApiMessage]

## Functionality

TODO:
- Wire message (Valid is not a valid wire message, it must contain one
invalid vote).
- Distribution of backing and approval votes, in case of nodes not having them.


### Distribution

Distributing disputes needs to be a reliable protocol. We would like to make as
sure as possible that our vote got properly delivered to all concerned
validators. For this to work, this subsystem won't be gossip based, but instead
will use a request/response protocol for application level confirmations. The
request will be the payload (the `ExplicitDisputeStatement`), the response will
be the confirmation. On reception of `DistributeStatement` a node will send and
keep retrying delivering that statement in case of failures as long as the
dispute is active to all concerned validators. The list of concerned validators
will be updated on every block and will change at session boundaries.

As can be determined from the protocol section, this subsystem is only concerned
with delivering `ExplicityDisputeStatement`s, for all other votes
(backing/approval) the dispute coordinator is responsible of keeping track of
those statements.

To cather with spam issues, we will in a first implementation only consider
disputes of already included data. Therefore only for candidates that are
already available. These are the only disputes representing an actual threat to
the system and are also the easiest to implement with regards to spam.

### Reception

Apart from making sure that local statements are sent out to all relevant
validators, this subsystem is also responsible for receiving votes from other
nodes. Because we are not forwarding foreign statements, spam is not so much of
an issue. We should just make sure to punish a node if it issues a statement for
a candidate that was not found available.

For each received vote, the subsystem will send an
`DisputeCoordinatorMessage::ImportStatements` message to the dispute
coordinator. We rely on the coordinator to trigger validation and availability
recovery of the candidate, if there was no local vote for it yet and to report
back to us via `DisputeDistributionMessage::ReportCandidateUnavailable` if a
candidate was not found available.

## Backing and Approval Votes

Backing and approval votes get imported when they arrive/are created via the
distpute coordinator by corresponding subsystems.

We assume that under normal operation each node will be aware of backing and
approval votes and optimize for that case. Nevertheless we want disputes to
conclude fast and reliable, therefore if a node is not aware of backing/approval
votes it can request the missing votes from the node that informed it about the
dispute. It will send the node two bitfields with


## Bitfields

Each validator is responsible for sending its vote to each other validator. For
backing and approval votes we assume each validator is aware of those. To cather
for imperfections, e.g.:

- A validator might have missed gossip and is not aware of
backing/approval votes
- A validator crashed/was under attack and was only able to
send out its vote to some validators

We also have a third wire message type: `IHaveVotes` which contains a bitfield
with a bit for each validator. 1 meaning we have some votes from that validator,
0 meaning we don't.

A validator might send those out to some validator whenever it feels like
missing out on votes and after some timeout just to make sure it knows
everything. The receiver of a `IHaveVotes` message will do two things:

1. See if the sender is missing votes we are aware of - if so, respond with
   those votes. Also send votes of equivocating validators, no matter the
   bitfield.
2. Check whether the sender knows about any votes, we don't know about and if so
   send a `IHaveVotes` request back, with our knowledge.

When to send `IHaveVotes` messages:

1. Whenever we receive an `Invalid`/`Valid` vote and we are not aware of any
   disagreeing votes. In this case we will just drop the message and send a
   `IHaveVotes` message to the validator we received the `Invalid`/`Valid`
   message from.
2. Once per block to some random validator as long as the dispute is active.
3. Whenever we learn something new via `IHaveVotes`, share that knowledge with
   two more `IHaveVotes` messages with random other validators.
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
happening somehow, there is no real harm done, there was no serious attack to
begin with.

Third: As candidates can be made up at will, we are susceptible to spam. Two
validators can continuously contradict each other. This will result in a slash
of one of them. Still it would be good to consider that possibility and make
sure the network will work properly in such an event.

[DistputeDistributionMessage]: ../../types/overseer-protocol.md#dispute-distribution-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
[DisputeParticipationMessage]: ../../types/overseer-protocol.md#dispute-participation-message
