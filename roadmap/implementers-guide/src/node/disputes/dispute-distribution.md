# Dispute Distribution

## Protocol

### Input

[`DisputeDistributionMessage`][DisputeDistributionMessage]

### Output

- [`DisputeCoordinatorMessage::ActiveDisputes`][DisputeParticipationMessage]
- [`DisputeCoordinatorMessage::ImportStatements`][DisputeParticipationMessage]
- [`RuntimeApiMessage`][RuntimeApiMessage]

## Functionality

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

### Considerations

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
