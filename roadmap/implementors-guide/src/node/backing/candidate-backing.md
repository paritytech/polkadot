# Candidate Backing

The Candidate Backing subsystem ensures every parablock considered for relay block inclusion has been seconded by at least one validator, and approved by a quorum. Parablocks for which no validator will assert correctness are discarded. If the block later proves invalid, the initial backers are slashable; this gives polkadot a rational threat model during subsequent stages.

Its role is to produce backable candidates for inclusion in new relay-chain blocks. It does so by issuing signed [`Statement`s](../../type-definitions.html#statement-type) and tracking received statements signed by other validators. Once enough statements are received, they can be combined into backing for specific candidates.

Note that though the candidate backing subsystem attempts to produce as many backable candidates as possible, it does _not_ attempt to choose a single authoritative one. The choice of which actually gets included is ultimately up to the block author, by whatever metrics it may use; those are opaque to this subsystem.

Once a sufficient quorum has agreed that a candidate is valid, this subsystem notifies the [Provisioner](../utility/provisioner.html), which in turn engages block production mechanisms to include the parablock.

## Protocol

The [Candidate Selection subsystem](candidate-selection.html) is the primary source of non-overseer messages into this subsystem. That subsystem generates appropriate [`CandidateBackingMessage`s](../../type-definitions.html#candidate-backing-message), and passes them to this subsystem.

This subsystem validates the candidates and generates an appropriate [`Statement`](../../type-definitions.html#statement-type). All `Statement`s are then passed on to the [Statement Distribution subsystem](statement-distribution.html) to be gossiped to peers. When this subsystem decides that a candidate is invalid, and it was recommended to us to second by our own Candidate Selection subsystem, a message is sent to the Candidate Selection subsystem with the candidate's hash so that the collator which recommended it can be penalized.

## Functionality

The subsystem should maintain a set of handles to Candidate Backing Jobs that are currently live, as well as the relay-parent to which they correspond.

### On Overseer Signal

* If the signal is an [`OverseerSignal`](../../type-definitions.html#overseer-signal)`::StartWork(relay_parent)`, spawn a Candidate Backing Job with the given relay parent, storing a bidirectional channel with the Candidate Backing Job in the set of handles.
* If the signal is an [`OverseerSignal`](../../type-definitions.html#overseer-signal)`::StopWork(relay_parent)`, cease the Candidate Backing Job under that relay parent, if any.

### On `CandidateBackingMessage`

* If the message corresponds to a particular relay-parent, forward the message to the Candidate Backing Job for that relay-parent, if any is live.

> big TODO: "contextual execution"
>
> * At the moment we only allow inclusion of _new_ parachain candidates validated by _current_ validators.
> * Allow inclusion of _old_ parachain candidates validated by _current_ validators.
> * Allow inclusion of _old_ parachain candidates validated by _old_ validators.
>
> This will probably blur the lines between jobs, will probably require inter-job communication and a short-term memory of recently backable, but not backed candidates.

## Candidate Backing Job

The Candidate Backing Job represents the work a node does for backing candidates with respect to a particular relay-parent.

The goal of a Candidate Backing Job is to produce as many backable candidates as possible. This is done via signed [`Statement`s](../../type-definitions.html#statement-type) by validators. If a candidate receives a majority of supporting Statements from the Parachain Validators currently assigned, then that candidate is considered backable.

### On Startup

* Fetch current validator set, validator -> parachain assignments from runtime API.
* Determine if the node controls a key in the current validator set. Call this the local key if so.
* If the local key exists, extract the parachain head and validation function for the parachain the local key is assigned to.

### On Receiving New Signed Statement

```rust
if let Statement::Seconded(candidate) = signed.statement {
  if candidate is unknown and in local assignment {
    spawn_validation_work(candidate, parachain head, validation function)
  }
}

// add `Seconded` statements and `Valid` statements to a quorum. If quorum reaches validator-group
// majority, send a `BlockAuthorshipProvisioning::BackableCandidate(relay_parent, Candidate, Backing)` message.
```

### Spawning Validation Work

```rust
fn spawn_validation_work(candidate, parachain head, validation function) {
  asynchronously {
    let pov = (fetch pov block).await

    // dispatched to sub-process (OS process) pool.
    let valid = validate_candidate(candidate, validation function, parachain head, pov).await;
    if valid {
      // make PoV available for later distribution. Send data to the availability store to keep.
      // sign and dispatch `valid` statement to network if we have not seconded the given candidate.
    } else {
      // sign and dispatch `invalid` statement to network.
    }
  }
}
```

### Fetch Pov Block

Create a `(sender, receiver)` pair.
Dispatch a `PovFetchSubsystemMessage(relay_parent, candidate_hash, sender)` and listen on the receiver for a response.

### On Receiving `CandidateBackingMessage`

* If the message is a `CandidateBackingMessage::RegisterBackingWatcher`, register the watcher and trigger it each time a new candidate is backable. Also trigger it once initially if there are any backable candidates at the time of receipt.
* If the message is a `CandidateBackingMessage::Second`, sign and dispatch a `Seconded` statement only if we have not seconded any other candidate and have not signed a `Valid` statement for the requested candidate. Signing both a `Seconded` and `Valid` message is a double-voting misbehavior with a heavy penalty, and this could occur if another validator has seconded the same candidate and we've received their message before the internal seconding request.

> TODO: send statements to Statement Distribution subsystem, handle shutdown signal from candidate backing subsystem
