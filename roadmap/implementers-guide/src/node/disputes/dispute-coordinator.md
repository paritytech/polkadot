# Dispute Coordinator

The coordinator is the central subsystem of the node-side components which
participate in disputes. It wraps a database, which is used to track statements
observed by _all_ validators over some window of sessions. Votes older than this
session window are pruned.

In particular the dispute-coordinator is responsible for:

- Ensuring that the node is able to raise a dispute in case an invalid candidate
  is found during approval checking.
- Ensuring approval votes will be recorded.
- Coordinating actual participation in a dispute, ensuring that the node
  participates in any justified dispute in a way that ensures resolution of
  disputes on the network even in the case of many disputes raised (flood/DoS
  scenario).
- Ensuring disputes resolve, even for candidates on abandoned forks as much as
  reasonably possible, to rule out "free tries" and thus guarantee our gambler's
  ruin property.
- Provide an API for chain selection, so we can prevent finalization of any
  chain which has included candidates for which a dispute is either ongoing or
  concluded invalid and avoid building on chains with an included invalid
  candidate.
- Provide an API for retrieving (resolved) disputes, including all votes, both
  implicit (approval, backing) and explicit dispute votes. So validators can get
  rewarded/slashed accordingly.
- Ensure backing votes are recorded and will never get overridden by explicit
  votes.

## Ensuring That Disputes Can Be Raised

If a candidate turns out invalid in approval checking, the `approval-voting`
subsystem will try to issue a dispute. For this, it will send a message
`DisputeCoordinatorMessage::IssueLocalStatement` to the dispute coordinator,
indicating to cast an explicit invalid vote. It is the responsibility of the
dispute coordinator on reception of such a message to create and sign that
explicit invalid vote and trigger a dispute if none for that candidate is
already ongoing.

In order to raise a dispute, a node has to be able to provide two opposing votes.
Given that the reason of the backing phase is to have validators with skin in
the game, the opposing valid vote will very likely be a backing vote. It could
also be some already cast approval vote, but the significant point here is: As
long as we have backing votes available, any node will be able to raise a
dispute.

Therefore a vital responsibility of the dispute coordinator is to make sure
backing votes are available for all candidates that might still get disputed. To
accomplish this task in an efficient way the dispute-coordinator relies on chain
scraping. Whenever a candidate gets backed on chain, we record in chain storage
the backing votes imported in that block. This way, given the chain state for a
given relay chain block, we can retrieve via a provided runtime API the backing
votes imported by that block. The dispute coordinator makes sure to query those
votes for any non finalized blocks: In case of missed blocks, it will do chain
traversal as necessary.

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

## Ensuring approval votes will be recorded

### Ensuring Recording

Only votes recorded by the dispute coordinator will be considered for slashing.

While there is no need to record approval votes in the dispute coordinator
preemptively, we make some effort to have any in approval-voting received
approval votes recorded when a dispute actually happens:

This is not required for concluding the dispute, as nodes send their own vote
anyway (either explicit valid or their existing approval-vote). What nodes can
do though, is participating in approval-voting, casting a vote, but later when a
dispute is raised reconsider their vote and send an explicit invalid vote. If
they managed to only have that one recorded, then they could avoid a slash.

This is not a problem for our basic security assumptions: The backers are the
ones to be supposed to have skin in the game, so we are not too woried about
colluding approval voters getting away slash free as the gambler's ruin property
is maintained anyway. There is however a separate problem, from colluding
approval-voters, that is "lazy" approval voters. If it were easy and reliable
for approval-voters to reconsider their vote, in case of an actual dispute, then
they don't have a direct incentive (apart from playing a part in securing the
network) to properly run the validation function at all - they could just always
vote "valid" totally risk free. (While they would alwasy risk a slash by voting
invalid.)


So we do want to fetch approval votes from approval-voting. Importing votes is
most efficient when batched. At the same time approval voting and disputes are
running concurrently so approval votes are expected to trickle in still, when a
dispute is already ongoing.

Hence, we have the following requirements for importing approval votes:

1. Only import them when there is a dispute, because otherwise we are
   wasting lots of resources _always_ for the exceptional case of a dispute.
2. Import votes batched when possible, to avoid quadratic import complexity.
3. Take into account that approval voting is still ongoing, while a dispute is
   already running.

With a design where approval voting sends votes to the dispute-coordinator by
itself, we would need to make approval voting aware of ongoing disputes and once
it is aware it could start sending all already existing votes batched and
trickling in votes as they come. The problem with this is, that it adds some
unnecessary complexity to approval-voting and also we might still import most of
the votes unbatched one-by-one, depending on what point in time the dispute was
raised.

Instead of the dispute coordinator informing approval-voting of an ongoing
dispute for it to begin forwarding votes to the dispute coordinator, it makes
more sense for the dispute-coordinator to just ask approval-voting for votes of
candidates in dispute. This way, the dispute coordinator can also pick the best
time for maximizing the number of votes in the batch.

Now the question remains, when should the dispute coordinator ask
approval-voting for votes?

In fact for slashing it is only relevant to have them once the dispute
concluded, so we can query approval voting the moment the dispute concludes!
Two concerns that come to mind, are easily addressed:

1. Timing: We would like to rely as little as possible on implementation details
   of approval voting. In particular, if the dispute is ongoing for a long time,
   do we have any guarantees that approval votes are kept around long enough by
   approval voting? Will approval votes still be present by the time the
   dispute concludes in all cases? The answer is nuanced, but in general we
   cannot rely on it. The problem is first, that finalization and
   approval-voting is an off-chain process so there is no global consensus: As
   soon as at least f+1 honest (f=n/3, where n is the number of
   validators/nodes) nodes have seen the dispute conclude, finalization will
   take place and approval votes will be cleared. This would still be fine, if
   we had some guarantees that those honest nodes will be able to include those
   votes in a block. This guarantee does not exist unfortunately, we will
   discuss the problem and solutions in more detail [below][#Ensuring Chain Import].

   The second problem is that approval-voting will abandon votes as soon as a
   chain can no longer be finalized (some other/better fork already has been).
   This second problem can somehow be mitigated by also importing votes as soon
   as a dispute is detected, but not fully resolved. It is still inherently
   racy. The good thing is, this should be good enough: We are worried about
   lazy approval checkers, the system does not need to be perfect. It should be
   enough if there is some risk of getting caught.
2. We are not worried about the dispute not concluding, as nodes will always
   send their own vote, regardless of it being an explict or an already existing
   approval-vote.

Conclusion: As long as we make sure, if our own approval vote gets imported
(which would prevent dispute participation) to also distribute it via
dispute-distribution, disputes can conclude. To mitigate raciness with
approval-voting deleting votes we will import approval votes twice during a
dispute: Once when it is raised, to make as sure as possible to see approval
votes also for abandoned forks and second when the dispute concludes, to
maximize the amount of potentially malicious approval votes to be recorded. The
raciness obviously is not fully resolved by this, but this is fine as argued
above.

Ensuring vote import on chain is covered in the next section.

What we don't care about is that honest approval-voters will likely validate
twice, once in approval voting and once via dispute-participation. Avoiding that
does not really seem worthwhile though, as disputes are for one exceptional, so
a little wasted effort won't affect everyday performance - second, even with
eager importing of approval votes, those doubled work is still present as
disputes and approvals are racing. Every time participation is faster than
approval, a node would do double work.

### Ensuring Chain Import

While in the previous section we discussed means for nodes to ensure relevant
votes are recorded so lazy approval checkers get slashed properly, it is crucial
to also discuss the actual chain import. Only if we guarantee that recorded votes
will get imported on chain (on all potential chains really) we will succeed
in executing slashes. Particularly we need to make sure backing votes end up on
chain consistently.

Dispute distribution will make sure all explicit dispute votes get distributed
among nodes which includes current block producers (current authority set) which
is an important property: If the dispute carries on across an era change, we
need to ensure that the new validator set will learn about any disputes and
their votes, so they can put that information on chain. Dispute-distribution
luckily has this property and always sends votes to the current authority set.
The issue is, for dispute-distribution, nodes send only their own explicit (or
in some cases their approval vote) in addition to some opposing vote. This
guarantees that at least some backing or approval vote will be present at the
block producer, but we don't have a 100% guarantee to have votes for all
backers, even less for approval checkers.

Reason for backing votes: While backing votes will be present on at least some
chain, that does not mean that any such chain is still considered for block
production in the current set - they might only exist on an already abandoned
fork. This means a block producer that just joined the set, might not have seen
any of them.

For approvals it is even more tricky and less necessary: Approval voting together
with finalization is a completely off-chain process therefore those protocols
don't care about block production at all. Approval votes only have a guarantee of
being propagated between the nodes that are responsible for finalizing the
concerned blocks. This implies that on an era change the current authority set,
will not necessarily get informed about any approval votes for the previous era.
Hence even if all validators of the previous era successfully recorded all approval
votes in the dispute coordinator, they won't get a chance to put them on chain,
hence they won't be considered for slashing.

It is important to note, that the essential properties of the system still hold:
Dispute-distribution will distribute at _least one_ "valid" vote to the current
authority set, hence at least one node will get slashed in case of outcome
"invalid". Also in reality the validator set is rarely exchanged 100%, therefore
in practice some validators in the current authority set will overlap with the
ones in the previous set and will be able to record votes on chain.

Still, for maximum accountability we need to make sure a previous authority set
can communicate votes to the next one, regardless of any chain: This is yet to
be implemented see section "Resiliency" in dispute-distribution and
[this](https://github.com/paritytech/polkadot/issues/3398) ticket.

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

### Participation

As explained, just blindly participating in any "dispute" that comes along is
not a good idea. First we would like to make sure the dispute is actually
genuine, to prevent cheap DoS attacks. Secondly, in case of genuine disputes, we
would like to conclude one after the other, in contrast to
processing all at the same time, slowing down progress on all of them, bringing
individual processing to a complete halt in the worst case (nodes get overwhelmed
at some stage in the pipeline).

To ensure to only spend significant work on genuine disputes, we only trigger
participation at all on any _vote import_ if any of the following holds true:

- We saw the disputed candidate included in some not yet finalized block on at
  least one fork of the chain.
- We have seen the disputed candidate backed in some not yet finalized block on
  at least one fork of the chain. This ensures the candidate is at least not
  completely made up and there has been some effort already flown into that
  candidate. Generally speaking a dispute shouldn't be raised for a candidate
  which is backed but is not yet included. Disputes are raised during approval
  checking. We participate on such disputes as a precaution - maybe we haven't
  seen the `CandidateIncluded` event yet?
- The dispute is already confirmed: Meaning that 1/3+1 nodes already
  participated, as this suggests in our threat model that there was at least one
  honest node that already voted, so the dispute must be genuine.

Note: A node might be out of sync with the chain and we might only learn about a
block, including a candidate, after we learned about the dispute. This means, we
have to re-evaluate participation decisions on block import!

With this, nodes won't waste significant resources on completely made up
candidates. The next step is to process dispute participation in a (globally)
ordered fashion. Meaning a majority of validators should arrive at at least
roughly at the same ordering of participation, for disputes to get resolved one
after another. This order is only relevant if there are lots of disputes, so we
obviously only need to worry about order if participations start queuing up.

We treat participation for candidates that we have seen included with priority
and put them on a priority queue which sorts participation based on the block
number of the relay parent of the candidate and for candidates with the same
relay parent height further by the `CandidateHash`. This ordering is globally
unique and also prioritizes older candidates.

The latter property makes sense, because if an older candidate turns out invalid,
we can roll back the full chain at once. If we resolved earlier disputes first
and they turned out invalid as well, we might need to roll back a couple of
times instead of just once to the oldest offender. This is obviously a good
idea, in particular it makes it impossible for an attacker to prevent rolling
back a very old candidate, by keeping raising disputes for newer candidates.

For candidates we have not seen included, but we know are backed (thanks to
chain scraping) or we have seen a dispute with 1/3+1 participation (confirmed
dispute) on them - we put participation on a best-effort queue. It has got the
same ordering as the priority one - by block heights of the relay parent, older
blocks are with priority. There is a possibility not to be able to obtain the
block number of the parent when we are inserting the dispute in the queue. To
account for races, we will promote any existing participation request to the
priority queue once we learn about an including block. NOTE: this is still work
in progress and is tracked by [this
issue](https://github.com/paritytech/polkadot/issues/5875).

### Abandoned Forks

Finalization: As mentioned we care about included and backed candidates on any
non-finalized chain, given that any disputed chain will not get finalized, we
don't need to care about finalized blocks, but what about forks that fall behind
the finalized chain in terms of block number? For those we would still like to
be able to participate in any raised disputes, otherwise attackers might be able
to avoid a slash if they manage to create a better fork after they learned about
the approval checkers. Therefore we do care about those forks even after they
have fallen behind the finalized chain.

For simplicity we also care about the actual finalized chain (not just forks) up
to a certain depth. We do have to limit the depth, because otherwise we open a
DoS vector again. The depth (into the finalized chain) should be oriented on the
approval-voting execution timeout, in particular it should be significantly
larger. Otherwise by the time the execution is allowed to finish, we already
dropped information about those candidates and the dispute could not conclude.

## Import

### Spam Considerations

In the last section we looked at how to treat queuing participations to
handle heavy dispute load well. This already ensures, that honest nodes won't
amplify cheap DoS attacks. There is one minor issue remaining: Even if we delay
participation until we have some confirmation of the authenticity of the
dispute, we should also not blindly import all votes arriving into the database
as this might be used to just slowly fill up disk space, until the node is no
longer functional. This leads to our last protection mechanism at the dispute
coordinator level (dispute-distribution also has its own), which is spam slots.
For each import containing an invalid vote, where we don't know whether it might
be spam or not we increment a counter for each signing participant of explicit
`invalid` votes.

What votes do we treat as a potential spam? A vote will increase a spam slot if
and only if all of the following conditions are satisfied:

* the candidate under dispute was not seen included nor backed on any chain
* the dispute is not confirmed
* we haven't cast a vote for the dispute

The reason this works is because we only need to worry about actual dispute
votes. Import of backing votes are already rate limited and concern only real
candidates for approval votes a similar argument holds (if they come from
approval-voting), but we also don't import them until a dispute already
concluded. For actual dispute votes, we need two opposing votes, so there must be
an explicit `invalid` vote in the import. Only a third of the validators can be
malicious, so spam disk usage is limited to `2*vote_size*n/3*NUM_SPAM_SLOTS`, with
`n` being the number of validators.

### Backing Votes

Backing votes are in some way special. For starters they are the only valid
votes that are guaranteed to exist for any valid dispute to be raised. Second
they are the only votes that commit to a shorter execution timeout
`BACKING_EXECUTION_TIMEOUT`, compared to a more lenient timeout used in approval
voting. To account properly for execution time variance across machines,
slashing might treat backing votes differently (more aggressively) than other
voting `valid` votes. Hence in import we shall never override a backing vote
with another valid vote. They can not be assumed to be interchangeable.

## Attacks & Considerations

The following attacks on the priority queue and best-effort queues are
considered in above design.

### Priority Queue

On the priority queue, we will only queue participations for candidates we have
seen included on any chain. Any attack attempt would start with a candidate
included on some chain, but an attacker could try to only reveal the including
relay chain blocks to just some honest validators and stop as soon as it learns
that some honest validator would have a relevant approval assignment.

Without revealing the including block to any honest validator, we don't really
have an attack yet. Once the block is revealed though, the above is actually
very hard. Each honest validator will re-distribute the block it just learned
about. This means an attacker would need to pull of a targeted DoS attack, which
allows the validator to send its assignment, but prevents it from forwarding and
sharing the relay chain block.

This sounds already hard enough, provided that we also start participation if
we learned about an including block after the dispute has been raised already
(we need to update participation queues on new leaves), but to be even safer
we choose to have an additional best-effort queue.

### Best-Effort Queue

While attacking the priority queue is already pretty hard, attacking the
best-effort queue is even harder. For a candidate to be a threat, it has to be
included on some chain. For it to be included, it has to have been backed before
and at least n/3 honest nodes must have seen that block, so availability
(inclusion) can be reached. Making a full third of the nodes not further
propagate a block, while at the same time allowing them to fetch chunks, sign
and distribute bitfields seems almost infeasible and even if accomplished, those
nodes would be enough to confirm a dispute and we have not even touched the
above fact that in addition, for an attack, the following including block must
be shared with honest validators as well.

It is worth mentioning that a successful attack on the priority queue as
outlined above is already outside of our threat model, as it assumes n/3
malicious nodes + additionally malfunctioning/DoSed nodes. Even more so for
attacks on the best-effort queue, as our threat model only allows for n/3
malicious _or_ malfunctioning nodes in total. It would therefore be a valid
decision to ditch the best-effort queue, if it proves to become a burden or
creates other issues.

One issue we should not be worried about though is spam. For abusing best-effort
for spam, the following scenario would be necessary:

An attacker controls a backing group: The attacker can then have candidates
backed and choose to not provide chunks. This should come at a cost to miss out
on rewards for backing, so is not free. At the same time it is rate limited, as
a backing group can only back so many candidates legitimately. (~ 1 per slot):

1. They have to wait until a malicious actor becomes block producer (for causing
  additional forks via equivocation for example).
2. Forks are possible, but if caused by equivocation also not free.
3. For each fork the attacker has to wait until the candidate times out, for
  backing another one.

Assuming there can only be a handful of forks, 2) together with 3) the candidate
timeout restriction, frequency should indeed be in the ballpark of once per
slot. Scaling linearly in the number of controlled backing groups, so two groups
would mean 2 backings per slot, ...

So by this reasoning an attacker could only do very limited harm and at the same
time will have to pay some price for it (it will miss out on rewards). Overall
the work done by the network might even be in the same ballpark as if actors
just behaved honestly:

1. Validators would have fetched chunks
2. Approval checkers would have done approval checks

While because of the attack (backing, not providing chunks and afterwards
disputing the candidate), the work for 1000 validators would be:

All validators sending out ~ 1000 tiny requests over already established
connections, with also tiny (byte) responses.

This means around a million requests, while in the honest case it would be ~
10000 (30 approval checkers x330) - where each request triggers a response in
the range of kilobytes. Hence network load alone will likely be higher in the
honest case than in the DoS attempt case, which would mean the DoS attempt
actually reduces load, while also costing rewards.

In the worst case this can happen multiple times, as we would retry that on
every vote import. The effect would still be in the same ballpark as honest
behavior though and can also be mitigated by chilling repeated availability
recovery requests for example.

## Out of Scope

### No Disputes for Non Included Candidates

We only ever care about disputes for candidates that have been included on at
least some chain (became available). This is because the availability system was
designed for precisely that: Only with inclusion (availability) we have
guarantees about the candidate to actually be available. Because only then we
have guarantees that malicious backers can be reliably checked and slashed. The
system was also designed for non included candidates to not pose any threat to
the system.

One could think of an (additional) dispute system to make it possible to dispute
any candidate that has been proposed by a validator, no matter whether it got
successfully included or even backed. Unfortunately, it would be very brittle
(no availability) and also spam protection would be way harder than for the
disputes handled by the dispute-coordinator. In fact all described spam handling
strategies above would simply be not available.

It is worth thinking about who could actually raise such disputes anyway:
Approval checkers certainly not, as they will only ever check once availability
succeeded. The only other nodes that meaningfully could/would are honest backing
nodes or collators. For collators spam considerations would be even worse as
there can be an unlimited number of them and we can not charge them for spam, so
trying to handle disputes raised by collators would be even more complex. For
honest backers: It actually makes more sense for them to wait until availability
is reached as well, as only then they have guarantees that other nodes will be
able to check. If they disputed before, all nodes would need to recover the data
from them, so they would be an easy DoS target.

In summary: The availability system was designed for raising disputes in a
meaningful and secure way after availability was reached. Trying to raise
disputes before does not meaningfully contribute to the systems security/might
even weaken it as attackers are warned before availability is reached, while at
the same time adding signficant amount of complexity. We therefore punt on such
disputes and concentrate on disputes the system was designed to handle.

### No Disputes for Already Finalized Blocks

Note that by above rules in the `Participation` section, we will not participate
in disputes concerning a candidate in an already finalized block. This is
because, disputing an already finalized block is simply too late and therefore
of little value. Once finalized, bridges have already processed the block for
example, so we have to assume the damage is already done. Governance has to step
in and fix what can be fixed.

Making disputes for already finalized blocks possible would only provide two
features:

1. We can at least still slash attackers.
2. We can freeze the chain to some governance only mode, in an attempt to
   minimize potential harm done.

Both seem kind of worthwhile, although as argued above, it is likely that there
is not too much that can be done in 2 and we would likely only ending up DoSing
the whole system without much we can do. 1 can also be achieved via governance
mechanisms.

In any case, our focus should be making as sure as reasonably possible that any
potentially invalid block does not get finalized in the first place. Not
allowing disputing already finalized blocks actually helps a great deal with
this goal as it massively reduces the amount of candidates that can be disputed.

This makes attempts to overwhelm the system with disputes significantly harder
and counter measures way easier. We can limit inclusion for example (as
suggested [here](https://github.com/paritytech/polkadot/issues/5898) in case of
high dispute load. Another measure we have at our disposal is that on finality
lag block production will slow down, implicitly reducing the rate of new
candidates that can be disputed. Hence, the cutting-off of the unlimited
candidate supply of already finalized blocks, guarantees the necessary DoS
protection and ensures we can have measures in place to keep up with processing
of disputes.

If we allowed participation for disputes for already finalized candidates, the
above spam protection mechanisms would be insufficient/relying 100% on full and
quick disabling of spamming validators.

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
