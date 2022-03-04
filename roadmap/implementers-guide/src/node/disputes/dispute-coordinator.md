# Dispute Coordinator

The coordinator is the central subsystem of the node-side components which participate in disputes. It
wraps a database, which used to track all statements observed by _all_ validators over some window
of sessions. Votes older than this session window are pruned.

This subsystem will be the point which produce dispute votes, either positive or negative, based on
locally-observed validation results as well as a sink for votes received by other subsystems. When
a statement import makes it clear that a dispute has been raised (there are opposing votes for a
candidate), the dispute coordinator will make sure the local node will participate in the dispute.

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

## Internal modules
Dispute coordinator subsystem includes a few internal modules - `ordering`, `participation` and
`spam_slots`.

### Ordering
Ordering module contains two structs - `OrderingProvider` and `CandidateComparator`. The former
keeps track of included blocks and their ancestors. It also generates `CandidateComparator`
instances for candidates.

`CandidateComparator` wraps the candidate hash and its parent block number:

```rust
pub struct CandidateComparator {
  /// Block number of the relay parent.
  ///
  /// Important, so we will be participating in oldest disputes first.
  ///
  /// Note: In theory it would make more sense to use the `BlockNumber` of the including
  /// block, as inclusion time is the actual relevant event when it comes to ordering. The
  /// problem is, that a candidate can get included multiple times on forks, so the `BlockNumber`
  /// of the including block is not unique. We could theoretically work around that problem, by
  /// just using the lowest `BlockNumber` of all available including blocks - the problem is,
  /// that is not stable. If a new fork appears after the fact, we would start ordering the same
  /// candidate differently, which would result in the same candidate getting queued twice.
  relay_parent_block_number: BlockNumber,
  /// By adding the `CandidateHash`, we can guarantee a unique ordering across candidates.
  candidate_hash: CandidateHash,
}
```

It also implements `PartialEq`, `Eq`, `PartialOrd` and `Ord` traits enabling comparison operations
with the comparators.

`Comparator` is used inside `Participation` module as a key for saving `ParticipationRequest`. It
provides the ordering required to process the most important requests first (check the next section
for details).

### Participation
This module keeps track of the disputes that the node participates in. At most there are
`MAX_PARALLEL_PARTICIPATIONS` parallel participations in the subsystem. The internal state of the
module is:

```rust
pub struct Participation {
  /// Participations currently being processed.
  running_participations: HashSet<CandidateHash>,
  /// Priority and best effort queues.
  queue: Queues,
  /// Sender to be passed to worker tasks.
  worker_sender: WorkerMessageSender,
  /// Some recent block for retrieving validation code from chain.
  recent_block: Option<(BlockNumber, Hash)>,
}
```
New candidates are processed immediately if the number of running participations is less than
`MAX_PARALLEL_PARTICIPATIONS` or queued for processing otherwise. `Participation` uses another
internal module `Queues` which provides prioritisation of the disputes. It guarantees that important
disputes will be processed first. The actual decision how important is a given dispute is performed
by the `ordering` module.

The actual participation is performed by `fn participate()`. First it sends
`AvailabilityRecoveryMessage::RecoverAvailableData` to obtain data from the validators. Then gets
the validation code and stores `AvailableData` with `AvailabilityStoreMessage::StoreAvailableData`
message.  Finally Participation module performs the actual validation and sends the result as
`WorkerMessage` to the main subsystem (`DisputeCoordinatorSubsystem`). `Participation` generates
messages which `DisputeCoordinatorSubsystem` consumes. You can find more information how these
events are processed in the next section.

### SpamSlots

`struct SpamSlots` aims to protect the validator from malicious peers generating erroneous disputes
with the purpose of overloading the validator with unnecessary work.

How the spam protection works? Each peer validator has got a spam slot for unconfirmed disputes with
fixed size (`MAX_SPAM_VOTES`). Each unconfirmed dispute is added to one such slot until all slots
for the given validator are filled up. At this point statements from this validator for unconfirmed
disputes are ignored.

What unconfirmed dispute means? Quote from the source code provides an excellent explanation:

> Node has not seen the candidate be included on any chain, it has not cast a
> vote itself on that dispute, the dispute has not yet reached more than a third of
> validator's votes and the including relay chain block has not yet been finalized.

`SpamSlots` has got this internal state:

```rust
pub struct SpamSlots {
  /// Counts per validator and session.
  ///
  /// Must not exceed `MAX_SPAM_VOTES`.
  slots: HashMap<(SessionIndex, ValidatorIndex), SpamCount>,

  /// All unconfirmed candidates we are aware of right now.
  unconfirmed: UnconfirmedDisputes,
}
```

It's worth noting that `SpamSlots` provides an interface for adding entries (`fn add_unconfirmed()`)
and removing them (`fn clear()`). The actual spam protection logic resides in the main subsystem, in
`fn handle_import_statements()`. It is invoked during `DisputeCoordinatorMessage::ImportStatements`
message handling (there is a dedicated section for it below). Spam slots are indexed by session id
and validator index. For each such pair there is a limit of active disputes. If this limit is
reached - the import is ignored.

Spam protection is performed only on invalid vote statements where the concerned candidate is not
included on any chain, not confirmed, not local and the votes hasn't reached the byzantine
threshold. This check is performed by `Ordering` module.

Spam slots are cleared when the session window advances so that the `SpamSlots` state doesn't grow
indefinitely.
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
* Initiate participation in disputes.
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
