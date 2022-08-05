# Dispute Coordinator

The coordinator is the central subsystem of the node-side components which participate in disputes. It
wraps a database, which used to track all statements observed by _all_ validators over some window
of sessions. Votes older than this session window are pruned.

In particular the dispute-coordinator is responsible for:

- Ensuring that the node is able to raise a dispute in case an invalid candidate
  is found during approval checking.
- Ensuring malicious approval votes will be recorded, so nodes can get slashed
  properly.
- Coordinating actual participation in a dispute, ensuring that the node
  participates in any justified dispute in a way that ensures resolution of
  disputes on the network even in the case of many disputes raised (flood/DoS
  scenario).
- Provide an API for chain selection, so we can prevent finalization of any
  chain which has included candidates for which a dispute is either ongoing or
  concluded invalid and avoid building on chains with an included invalid
  candidate.
- Provide an API for retrieving (resolved) disputes, including all votes, both
  implicit (approval, backing) and explicit dispute votes. So validators can get
  rewarded/slashed accordingly.

## Ensuring That Disputes Can Be Raised

If a candidate turns out invalid in approval checking, the `approval-voting`
subsystem will try to issue a dispute. For this, it will send a message
`DisputeCoordinatorMessage::IssueLocalStatement` to the dispute coordinator,
indicating to cast an explicit invalid vote. It is the responsibility of the
dispute coordinator on reception of such a message to create and sign that
explicit invalid vote and  trigger a dispute if none is already
ongoing.

In order to raise a dispute, a node has to be able to provide two opposing votes.
Given that the reason of the backing phase is to have validators with skin in
the game, the opposing valid vote will very likely be a backing vote. It could
also be some already cast approval vote, but the significant point here is: As
long as we have backing votes available, any node will be able to raise a
dispute.

Therefore a vital responsibility of the dispute coordinator is to make sure backing
votes are available for all candidates that might still get disputed. To
accomplish this task in an efficient way the dispute-coordinator relies on chain
scraping. Whenever a candidate gets backed on chain, we record in
chain storage the backing votes (gets overridden on every block). We provide a
runtime API for querying those votes. The dispute coordinator makes sure to
query those votes for any non finalized blocks: In case of missed blocks, it
will do chain traversal as necessary.

Relying on chain scraping is very efficient for two reasons:

1. Votes are already batched. We import all available backing votes for a
   candidate all at once. If instead we imported votes from candidate-backing as
   they came along, we would import each vote individually which is
   inefficient in the current dispute coordinator implementation (quadratic
   complexity).
2. We also import less votes in total, as we avoid importing statements for
   candidates that never got successfully backed on any chain.

It also is secure, because disputes are only ever raised in the approval voting
phase. A node only starts the approval process after it has seen a candidate
included on some chain, for that to happen it must have been backed previously.
Therefore backing votes are available at that point in time. Signals are
processed first, so even if a block is skipped and we only start importing
backing votes on the including block, we will have seen the backing votes by the
time we process messages from approval voting.

In summary, for making it possible for a dispute to be raised, recording of
backing votes from chain is sufficient and efficient. In particular there is no
need to preemptively import approval votes, which has shown to be a very
inefficient process. (Quadratic complexity adds up, with 35 votes in total per candidate)

Approval votes are very relevant nonetheless as we are going to see in the next
section.

## Ensuring Malicious Approval Votes Will Be Recorded

While there is no need to record approval votes in the dispute coordinator
preemptively, we do need to make sure they are recorded when a dispute
actually happens. This is because only votes recorded by the dispute
coordinator will be considered for slashing. While the backing group always gets
slashed, a serious attack attempt will likely also consist of malicious approval
checkers which will cast approval votes, although the candidate is invalid. If
we did not import those votes, those nodes would likely cast an `invalid` explicit
vote as part of the dispute in addition to their approval vote and thus avoid a
slash. With the 2/3rd honest assumption it seems unrealistic that malicious
actors will keep sending approval votes once they became aware of a raised
dispute. Hence the most crucial approval votes to import are the early ones
(tranche 0), to take into account network latencies and such we still want to
import approval votes at a later point in time as well (in particular we need to
make sure the dispute can conclude, but more on that later).

As mentioned already previously, importing votes is most efficient when batched.
At the same time approval voting and disputes are running concurrently so
approval votes are expected to trickle in still, when a dispute is already
ongoing.

Hence, we have the following requirements for importing approval votes:

1. Only import them when there is a dispute, because otherwise we are
   wasting lots of resources _always_ for the exceptional case of a dispute.
2. Import votes batched when possible, to avoid quadratic import complexity.
3. Take into account that approval voting is still ongoing, while a dispute is
   already running.

With a design where approval voting sends votes to the dispute-coordinator by
itself, we would need to make approval voting aware of ongoing disputes and
once it is aware it could start sending all already existing votes batched and
trickling in votes as they come. The problem with this is, that it adds some
unnecessary complexity to approval voting and also we might still import most of
the votes unbatched, but one-by-one, depending on what point in time the dispute
was raised.

Instead of the dispute coordinator telling approval-voting that a dispute is
ongoing for approval-voting to start sending votes to the dispute coordinator,
it would make more sense if the dispute-coordinator would just ask
approval-voting for votes of candidates that are currently disputed. This way
the dispute-coordinator can also pick the time when to ask and we can therefore
maximize the amount of batching.

Now the question remains, when should the dispute coordinator ask
approval-voting for votes? As argued above already, querying approval votes at
the beginning of the dispute, will likely already take care of most malicious
votes. Still we would like to have a record of all, if possible. So what are
other points in time we might query approval votes?

In fact for slashing it is only relevant to have them once the dispute
concluded, so we can query approval voting the moment the dispute concludes!
There are two potential caveats with this though:

1. Timing: We would like to rely as little as possible on implementation details
   of approval voting. In particular, if the dispute is ongoing for a long time,
   do we have any guarantees that approval votes are kept around long enough by
   approval voting? So will approval votes still be present by the time the
   dispute concludes in all cases? The answer should luckily be yes: As long as
   the chain is not finalized, which has to be the case once we have an ongoing
   dispute, approval votes have to be kept around (and distributed) otherwise we
   might not be able to finalize in case the validator set changes for example.
   Conclusively we can rely on approval votes to be still available when the
   dispute concludes.
2. There could be a chicken and egg problem: If we wait for approval vote import
   for the dispute to conclude, we would run into a problem if we needed those
   approval votes to get enough votes to conclude the dispute. Luckily it turns
   out that this is not quite true or at least can be made not true easily: As
   already mentioned, approval voting and disputes are running concurrently, but
   not only that, they race with each other! A node might simultaneously start
   participating in a dispute via the dispute coordinator, due to learning about
   a dispute via dispute-distribution, while also participating in
   approval voting. So if we don't import approval votes before the dispute
   concluded, we actually are making sure that no local vote is present and any
   honest node will cast an explicit vote in addition to its approval vote: The
   dispute can conclude! Then, by importing approval votes, we are ensuring the
   one missing property, that malicious approval voters will get slashed, even
   if they also cast an invalid explicit vote.

Conclusion: If we only ever import approval votes once a dispute concludes, then
nodes will send explicit votes and we will be able to conclude the dispute. This
indeed means some wasted effort, as in case of a dispute that concludes valid,
honest nodes will validate twice, once in approval voting and once via
dispute-participation. Avoiding that does not really seem worthwhile though, as
disputes are for one exceptional, so a little wasted effort won't affect
everyday performance - second, even if we imported approval votes, those doubled
work is still present as disputes and approvals are racing. Every time
participation is faster than approval, a node would do double work anyway.

One gotcha remains: We could be receiving our own approval vote via
dispute-distribution (or dispute chain scraping), because some (likely
malicious) node picked it as the opposing valid vote e.g. as an attempt to
prevent the dispute from concluding (it is only sending it to us).
The solution is simple though: When checking for an existing own vote to
determine whether or not to participate, we will instruct `dispute-distribution`
to distribute an already existing own approval vote. This way a dispute will
always be able to conclude, even with these kinds of attacks. Alternatively or
in addition to be double safe, we could also choose to simply drop (own)
approval votes from any import that is not requested from the
dispute-coordinator itself.

Side note: In fact with both of these we would already be triple safe, because
the dispute coordinator also scrapes any votes from ongoing disputes off chain.
Therefore, as soon as the current node becomes a block producer it will put its
own approval vote on chain, and all other honest nodes will retrieve it from
there.

## Coordinating Actual Dispute Participation

Once the dispute coordinator learns about a dispute, it is its responsibility to
make sure the local node participates in that dispute.

The dispute coordinator learns about a dispute by importing votes from either
chain scraping or from dispute-distribution. If it finds opposing votes (always
the case when coming from dispute-distribution), it records the presence of a
dispute. Then, in case it does not find any local vote for that dispute already,
it needs to trigger participation in the dispute (see previous section for
considerations when the found local vote is an approval vote).

Participation means, recovering availability and re-evaluating the POV. The
result of that validation (either valid or invalid) will be the node's vote on
that dispute: Either explicit "invalid" or "valid". The dispute coordinator will
inform `dispute-distribution` about our vote and `dispute-distribution` will make
sure that our vote gets distributed to all other validators.

Nothing ever is that easy though. We can not blindly import anything that comes
along and trigger participation no matter what.

### Spam Considerations

In Polkadot's security model, it is important that attempts to attack the system
result in a slash of the offenders. Therefore we need to make sure that this
slash is actually happening. Attackers could try to prevent the slashing from
taking place, by overwhelming validators with disputes in such a way that no
single dispute ever concludes, because nodes are busy processing newly incoming
ones. Other attacks are imaginable as well, like raising disputes for candidates
that don't exist, just filling up everyone's disk slowly or worse making nodes
try to participate, which will result in lots of network requests for recovering
availability.

The last point brings up a significant consideration in general: Disputes are
about escalation: Every node will suddenly want to check, instead of only a few.
A single message will trigger the whole network to start significant amount of
work and will cause lots of network traffic and messages. Hence the
dispute system is very susceptible to being a brutal amplifier for DoS attacks,
resulting in DoS attacks to become very easy and cheap, if we are not careful.

One counter measure we are taking is making raising of disputes a costly thing:
If you raise a dispute, because you claim a candidate is invalid, although it is
in fact valid - you will get slashed, hence you pay for consuming those
resources. The issue is: This only works if the dispute concerns a candidate
that actually exists!

If a node raises a dispute for a candidate that never got included (became
available) on any chain, then the dispute can never conclude, hence nobody gets
slashed. It makes sense to point out that this is less bad than it might sound
at first, as trying to participate in a dispute for a non existing candidate is
"relatively" cheap. Each node will send out a few hundred tiny request messages
for availability chunks, which all will end up in a tiny response "NoSuchChunk"
and then no participation will actually happen as there is nothing to
participate. Malicious nodes could provide chunks, which would make things more
costly, but at the full expense of the attackers bandwidth - no amplification
here. I am bringing that up for completeness only: Triggering a thousand nodes
to send out a thousand tiny network messages by just sending out a single
garbage message, is still a significant amplification and is nothing to ignore -
this could absolutely be used to cause harm!

#### Participation

As explained, just blindly participating in any "dispute" that comes along is
not a good idea. First we would like to make sure the dispute is actually
genuine, to prevent cheap DoS attacks. Secondly, in case of genuine disputes, we
would like to be able to be able to conclude one after the other, in contrast to
processing all at the same time, slowing down progress on all of them, bringing
individual processing to a complete halt in the worst case (nodes get overwhelmed
at some stage in the pipeline).

To ensure to only spend significant work on genuine disputes, we only trigger
participation at all on any _vote import_ if any of the following holds true:

- We saw the disputed candidate included on at least one fork of the chain
- We have "our" availability chunk available for that candidate as this suggests
  that either availability was at least run, although it might not have
  succeeded or we have been a backing node of the candidate. In both cases the
  candidate is at least not completely made up and there has been some effort
  already flown into that candidate.
- The dispute is already confirmed: Meaning that 1/3+1 nodes already
  participated, as this suggests in our threat model that there was at least one
  honest node that already voted, so the dispute must be genuine.

Note: A node might be out of sync with the chain and we might only learn about a
block including a candidate, after we learned about the dispute. This means, we
have to re-evaluate participation decisions on block import!

With this nodes won't waste significant resources on completely made up
candidates. The next step is to process dispute participation in a (globally)
ordered fashion. Meaning a majority of validators should arrive at at least
roughly at the same ordering of participation, for disputes to get resolved one
after another. This order is only relevant if there are lots of disputes, so we
obviously only need to worry about order if participations start queuing up.

We treat participation for candidates that we have seen included with priority
and put them on a priority queue which sorts participation based on the block
number of the relay parent of that candidate and for candidates with the same
relay parent height further by the `CandidateHash`. This ordering is globally
unique and also prioritizes older candidates.

The later property makes sense, because if an older candidate turns out invalid,
we can roll back the full chain at once. If we resolved earlier disputes first
and they turned out invalid as well, we might need to roll back a couple of
times instead of just once to the oldest offender. This is obviously a good
idea, in particular it makes it impossible for an attacker to prevent rolling
back a very old candidate, by keeping raising disputes for newer candidates.

For candidates we have not seen included, but we have our availability piece
available we put participation on a best-effort queue, which at the moment is
processed on the basis how often we requested participation locally, which
equals the number of times we imported votes for that dispute. The idea is, if
we have not seen the candidate included, but the dispute is valid, other nodes
will have seen it included - so the more votes there are, the more likely it is
a valid dispute and we should implicitly arrive at a similar ordering as the
nodes that are able to sort based on the relay parent block height.

#### Import

In the last section we looked at how to treat queuing participations to handle
heavy dispute load well. This already ensures, that honest nodes won't amplify
cheap DoS attacks. There is one minor issue remaining: Even if we delay
participation until we have some confirmation of the authenticity of the
dispute, we should also not blindly import all votes arriving into the
database as this might be used to just slowly fill up disk space, until the node
is no longer functional. This leads to our last protection mechanism at the
dispute coordinator level (dispute-distribution also has its own), which is spam
slots. For each import, where we don't know whether it might be spam or not we
increment a counter for each signing participant of explicit `invalid` votes.

The reason this works is because we only need to worry about actual dispute
votes. Import of backing votes are already rate limited and concern only real
candidates for approval votes a similar argument holds (if they come from
approval-voting), but we also don't import them until a dispute already
concluded. For actual dispute votes, we need two opposing votes, so there must be
an explicit `invalid` vote in the import. Only a third of the validators can be
malicious, so spam disk usage is limited to ```2*vote_size*n/3*NUM_SPAM_SLOTS```, with
n being the number of validators.
-
More reasoning behind spam considerations can be found on
[this](https://github.com/paritytech/srlabs_findings/issues/179) sr-lab ticket.

## Database Schema

We use an underlying Key-Value database where we assume we have the following operations available:
  * `write(key, value)`
  * `read(key) -> Option<value>`
  * `iter_with_prefix(prefix) -> Iterator<(key, value)>` - gives all keys and values in
    lexicographical order where the key starts with `prefix`.

We use this database to encode the following schema:

```rust
("candidate-votes", SessionIndex, CandidateHash) -> Option<CandidateVotes>
"recent-disputes" -> RecentDisputes
"earliest-session" -> Option<SessionIndex>
```

The meta information that we track per-candidate is defined as the `CandidateVotes` struct.
This draws on the [dispute statement types][DisputeTypes]

```rust
/// Tracked votes on candidates, for the purposes of dispute resolution.
pub struct CandidateVotes {
  /// The receipt of the candidate itself.
  pub candidate_receipt: CandidateReceipt,
  /// Votes of validity, sorted by validator index.
  pub valid: Vec<(ValidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
  /// Votes of invalidity, sorted by validator index.
  pub invalid: Vec<(InvalidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
}

/// The mapping for recent disputes; any which have not yet been pruned for being ancient.
pub type RecentDisputes = std::collections::BTreeMap<(SessionIndex, CandidateHash), DisputeStatus>;

/// The status of dispute. This is a state machine which can be altered by the
/// helper methods.
pub enum DisputeStatus {
  /// The dispute is active and unconcluded.
  Active,
  /// The dispute has been concluded in favor of the candidate
  /// since the given timestamp.
  ConcludedFor(Timestamp),
  /// The dispute has been concluded against the candidate
  /// since the given timestamp.
  ///
  /// This takes precedence over `ConcludedFor` in the case that
  /// both are true, which is impossible unless a large amount of
  /// validators are participating on both sides.
  ConcludedAgainst(Timestamp),
  /// Dispute has been confirmed (more than `byzantine_threshold` have already participated/ or
  /// we have seen the candidate included already/participated successfully ourselves).
  Confirmed,
}
```

## Protocol

Input: [`DisputeCoordinatorMessage`][DisputeCoordinatorMessage]

Output:
  - [`RuntimeApiMessage`][RuntimeApiMessage]

## Functionality

This assumes a constant `DISPUTE_WINDOW: SessionWindowSize`. This should correspond to at least 1
day.

Ephemeral in-memory state:

```rust
struct State {
  keystore: Arc<LocalKeystore>,
  rolling_session_window: RollingSessionWindow,
  highest_session: SessionIndex,
  spam_slots: SpamSlots,
  participation: Participation,
  ordering_provider: OrderingProvider,
  participation_receiver: WorkerMessageReceiver,
  metrics: Metrics,
  // This tracks only rolling session window failures.
  // It can be a `Vec` if the need to track more arises.
  error: Option<SessionsUnavailable>,
  /// Latest relay blocks that have been successfully scraped.
  last_scraped_blocks: LruCache<Hash, ()>,
}
```

### On startup
When the subsystem is initialised it waits for a new leaf (message `OverseerSignal::ActiveLeaves`).
The leaf is used to initialise a `RollingSessionWindow` instance (contains leaf hash and
`DISPUTE_WINDOW` which is a constant.

Next the active disputes are loaded from the DB. The subsystem checks if there are disputes for
which a local statement is not issued. A list of these is passed to the main loop.

### The main loop

Just after the subsystem initialisation the main loop (`fn run_until_error()`) runs until
`OverseerSignal::Conclude` signal is received. Before executing the actual main loop the leaf and
the participations, obtained during startup are enqueued for processing. If there is capacity (the
number of running participations is less than `MAX_PARALLEL_PARTICIPATIONS`) participation jobs are
started (`func participate`). Finally the component waits for messages from Overseer. The behaviour
on each message is described in the following subsections.

### On `OverseerSignal::ActiveLeaves`

Initiates processing via the `Participation` module and updates the internal state of the subsystem.
More concretely:

* Passes the `ActiveLeavesUpdate` message to the ordering provider.
* Updates the session info cache.
* Updates `self.highest_session`.
* Prunes old spam slots in case the session window has advanced.
* Scrapes on chain votes.

### On `MuxedMessage::Participation`

This message is sent from `Participatuion` module and indicates a processed dispute participation.
It's the result of the processing job initiated with `OverseerSignal::ActiveLeaves`. The subsystem
issues a `DisputeMessage` with the result.

### On `OverseerSignal::Conclude`

Exit gracefully.

### On `OverseerSignal::BlockFinalized`

Performs cleanup of the finalized candidate.

### On `DisputeCoordinatorMessage::ImportStatements`

Import statements by validators are processed in `fn handle_import_statements()`. The function has
got three main responsibilities:
* Initiate participation in disputes and sending out of any existing own
  approval vote in case of a raised dispute.
* Persist all fresh votes in the database. Fresh votes in this context means votes that are not
  already processed by the node.
* Spam protection on all invalid (`DisputeStatement::Invalid`) votes. Please check the SpamSlots
  section for details on how spam protection works.

### On `DisputeCoordinatorMessage::RecentDisputes`

Returns all recent disputes saved in the DB.

### On `DisputeCoordinatorMessage::ActiveDisputes`

Returns all recent disputes concluded within the last `ACTIVE_DURATION_SECS` .

### On `DisputeCoordinatorMessage::QueryCandidateVotes`

Loads `candidate-votes` for every `(SessionIndex, CandidateHash)` in the input query and returns
data within each `CandidateVote`. If a particular `candidate-vote` is missing, that particular
request is omitted from the response.

### On `DisputeCoordinatorMessage::IssueLocalStatement`

Executes `fn issue_local_statement()` which performs the following operations:

* Deconstruct into parts `{ session_index, candidate_hash, candidate_receipt, is_valid }`.
* Construct a [`DisputeStatement`][DisputeStatement] based on `Valid` or `Invalid`, depending on the
  parameterization of this routine.
* Sign the statement with each key in the `SessionInfo`'s list of parachain validation keys which is
  present in the keystore, except those whose indices appear in `voted_indices`. This will typically
  just be one key, but this does provide some future-proofing for situations where the same node may
  run on behalf multiple validators. At the time of writing, this is not a use-case we support as
  other subsystems do not invariably   provide this guarantee.
* Write statement to DB.
* Send a `DisputeDistributionMessage::SendDispute` message to get the vote distributed to other
  validators.

### On `DisputeCoordinatorMessage::DetermineUndisputedChain`

Executes `fn determine_undisputed_chain()` which performs the following:

* Load `"recent-disputes"`.
* Deconstruct into parts `{ base_number, block_descriptions, rx }`
* Starting from the beginning of `block_descriptions`:
  1. Check the `RecentDisputes` for a dispute of each candidate in the block description.
  1. If there is a dispute which is active or concluded negative, exit the loop.
* For the highest index `i` reached in the `block_descriptions`, send `(base_number + i + 1,
  block_hash)` on the channel, unless `i` is 0, in which case `None` should be sent. The
  `block_hash` is determined by inspecting `block_descriptions[i]`.

[DisputeTypes]: ../../types/disputes.md
[DisputeStatement]: ../../types/disputes.md#disputestatement
[DisputeCoordinatorMessage]: ../../types/overseer-protocol.md#dispute-coordinator-message
[RuntimeApiMessage]: ../../types/overseer-protocol.md#runtime-api-message
