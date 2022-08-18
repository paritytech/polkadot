# Candidate Backing

The Candidate Backing subsystem ensures every parablock considered for relay block inclusion has been seconded by at least one validator, and approved by a quorum. Parablocks for which not enough validators will assert correctness are discarded. If the block later proves invalid, the initial backers are slashable; this gives polkadot a rational threat model during subsequent stages.

Its role is to produce backable candidates for inclusion in new relay-chain blocks. It does so by issuing signed [`Statement`s][Statement] and tracking received statements signed by other validators. Once enough statements are received, they can be combined into backing for specific candidates.

Note that though the candidate backing subsystem attempts to produce as many backable candidates as possible, it does _not_ attempt to choose a single authoritative one. The choice of which actually gets included is ultimately up to the block author, by whatever metrics it may use; those are opaque to this subsystem.

Once a sufficient quorum has agreed that a candidate is valid, this subsystem notifies the [Provisioner][PV], which in turn engages block production mechanisms to include the parablock.

## Protocol

Input: [`CandidateBackingMessage`][CBM]

Output:

- [`CandidateValidationMessage`][CVM]
- [`RuntimeApiMessage`][RAM]
- [`CollatorProtocolMessage`][CPM]
- [`ProvisionerMessage`][PM]
- [`AvailabilityDistributionMessage`][ADM]
- [`StatementDistributionMessage`][SDM]

## Functionality

The [Collator Protocol][CP] subsystem is the primary source of non-overseer messages into this subsystem. That subsystem generates appropriate [`CandidateBackingMessage`s][CBM] and passes them to this subsystem.

This subsystem requests validation from the [Candidate Validation][CV] and generates an appropriate [`Statement`][Statement]. All `Statement`s are then passed on to the [Statement Distribution][SD] subsystem to be gossiped to peers. When [Candidate Validation][CV] decides that a candidate is invalid, and it was recommended to us to second by our own [Collator Protocol][CP] subsystem, a message is sent to the [Collator Protocol][CP] subsystem with the candidate's hash so that the collator which recommended it can be penalized.

The subsystem should maintain a set of handles to Candidate Backing Jobs that are currently live, as well as the relay-parent to which they correspond.

### On Overseer Signal

* If the signal is an [`OverseerSignal`][OverseerSignal]`::ActiveLeavesUpdate`:
  * spawn a Candidate Backing Job for each `activated` head referring to a fresh leaf, storing a bidirectional channel with the Candidate Backing Job in the set of handles.
  * cease the Candidate Backing Job for each `deactivated` head, if any.
* If the signal is an [`OverseerSignal`][OverseerSignal]`::Conclude`: Forward conclude messages to all jobs, wait a small amount of time for them to join, and then exit.

### On Receiving `CandidateBackingMessage`

* If the message is a [`CandidateBackingMessage`][CBM]`::GetBackedCandidates`, get all backable candidates from the statement table and send them back.
* If the message is a [`CandidateBackingMessage`][CBM]`::Second`, sign and dispatch a `Seconded` statement only if we have not seconded any other candidate and have not signed a `Valid` statement for the requested candidate. Signing both a `Seconded` and `Valid` message is a double-voting misbehavior with a heavy penalty, and this could occur if another validator has seconded the same candidate and we've received their message before the internal seconding request.
* If the message is a [`CandidateBackingMessage`][CBM]`::Statement`, count the statement to the quorum. If the statement in the message is `Seconded` and it contains a candidate that belongs to our assignment, request the corresponding `PoV` from the backing node via `AvailabilityDistribution` and launch validation. Issue our own `Valid` or `Invalid` statement as a result.

If the seconding node did not provide us with the `PoV` we will retry fetching from other backing validators.


> big TODO: "contextual execution"
>
> * At the moment we only allow inclusion of _new_ parachain candidates validated by _current_ validators.
> * Allow inclusion of _old_ parachain candidates validated by _current_ validators.
> * Allow inclusion of _old_ parachain candidates validated by _old_ validators.
>
> This will probably blur the lines between jobs, will probably require inter-job communication and a short-term memory of recently backable, but not backed candidates.

## Candidate Backing Job

The Candidate Backing Job represents the work a node does for backing candidates with respect to a particular relay-parent.

The goal of a Candidate Backing Job is to produce as many backable candidates as possible. This is done via signed [`Statement`s][STMT] by validators. If a candidate receives a majority of supporting Statements from the Parachain Validators currently assigned, then that candidate is considered backable.

### On Startup

* Fetch current validator set, validator -> parachain assignments from [`Runtime API`][RA] subsystem using [`RuntimeApiRequest::Validators`][RAM] and [`RuntimeApiRequest::ValidatorGroups`][RAM]
* Determine if the node controls a key in the current validator set. Call this the local key if so.
* If the local key exists, extract the parachain head and validation function from the [`Runtime API`][RA] for the parachain the local key is assigned to by issuing a [`RuntimeApiRequest::Validators`][RAM]
* Issue a [`RuntimeApiRequest::SigningContext`][RAM] message to get a context that will later be used upon signing.

### On Receiving New Candidate Backing Message

```rust
match msg {
  GetBackedCandidates(hashes, tx) => {
    // Send back a set of backable candidates.
  }
  CandidateBackingMessage::Second(hash, candidate) => {
    if candidate is unknown and in local assignment {
      if spawn_validation_work(candidate, parachain head, validation function).await == Valid {
        send(DistributePoV(pov))
      }
    }
  }
  CandidateBackingMessage::Statement(hash, statement) => {
    // count to the votes on this candidate
    if let Statement::Seconded(candidate) = statement {
      if candidate.parachain_id == our_assignment {
        spawn_validation_work(candidate, parachain head, validation function)
      }
    }
  }
}
```

Add `Seconded` statements and `Valid` statements to a quorum. If the quorum reaches a pre-defined threshold, send a [`ProvisionerMessage`][PM]`::ProvisionableData(ProvisionableData::BackedCandidate(CandidateReceipt))` message.
`Invalid` statements that conflict with already witnessed `Seconded` and `Valid` statements for the given candidate, statements that are double-votes, self-contradictions and so on, should result in issuing a [`ProvisionerMessage`][PM]`::MisbehaviorReport` message for each newly detected case of this kind.

Backing does not need to concern itself with providing statements to the dispute
coordinator as the dispute coordinator scrapes them from chain. This way the
import is batched and contains only statements that actually made it on some
chain.

### Validating Candidates.

```rust
fn spawn_validation_work(candidate, parachain head, validation function) {
  asynchronously {
    let pov = (fetch pov block).await

    let valid = (validate pov block).await;
    if valid {
      // make PoV available for later distribution. Send data to the availability store to keep.
      // sign and dispatch `valid` statement to network if we have not seconded the given candidate.
    } else {
      // sign and dispatch `invalid` statement to network.
    }
  }
}
```

### Fetch PoV Block

Create a `(sender, receiver)` pair.
Dispatch a [`AvailabilityDistributionMessage`][ADM]`::FetchPoV{ validator_index, pov_hash, candidate_hash, tx, } and listen on the passed receiver for a response. Availability distribution will send the request to the validator specified by `validator_index`, which might not be serving it for whatever reasons, therefore we need to retry with other backing validators in that case.


### Validate PoV Block

Create a `(sender, receiver)` pair.
Dispatch a `CandidateValidationMessage::Validate(validation function, candidate, pov, BACKING_EXECUTION_TIMEOUT, sender)` and listen on the receiver for a response.

### Distribute Signed Statement

Dispatch a [`StatementDistributionMessage`][SDM]`::Share(relay_parent, SignedFullStatement)`.

[OverseerSignal]: ../../types/overseer-protocol.md#overseer-signal
[Statement]: ../../types/backing.md#statement-type
[STMT]: ../../types/backing.md#statement-type
[CPM]: ../../types/overseer-protocol.md#collator-protocol-message
[RAM]: ../../types/overseer-protocol.md#runtime-api-message
[CVM]: ../../types/overseer-protocol.md#validation-request-type
[PM]: ../../types/overseer-protocol.md#provisioner-message
[CBM]: ../../types/overseer-protocol.md#candidate-backing-message
[ADM]: ../../types/overseer-protocol.md#availability-distribution-message
[SDM]: ../../types/overseer-protocol.md#statement-distribution-message
[DCM]: ../../types/overseer-protocol.md#dispute-coordinator-message

[CP]: ../collators/collator-protocol.md
[CV]: ../utility/candidate-validation.md
[SD]: statement-distribution.md
[RA]: ../utility/runtime-api.md
[PV]: ../utility/provisioner.md
