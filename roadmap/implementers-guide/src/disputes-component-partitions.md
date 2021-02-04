# Dispute Component Partitions

## Understanding fork awareness

There are parts regarding storage. Node side, runtime side and state root persistence.

* Dispute resolution must be persisted in the state-root of each and every fork.
* Dispute resolution.

## Duties

### VotesDB

#### Node

* Stores all cast votes on the node side.
* Allows other subsystems to query disputes
  * by `ValidatorId`
  * by `ValidatorIndex` and `SessionIndex`
  * by `SessionIndex`

* Cleans up votes based on session index

#### StateRoot

Does not interact with the state root.

#### Runtime

Has no interaction with the runtime.

### Participation

#### Node

Receives messages from

* network gossip containing..
  * individual `Vote`s or sets of `Votes`
  * `Code` + `PoV` as a response to such as request
  * a request to another validator (from which a vote was received before) to send `Code` + `PoV` our way

Sends messages

* to the `VotesDB` to ..
  * store `Vote`s
  * query `Vote`s for a particular

* to the network for ..
  * requesting `Code` + `PoV`
  * broadcasting received `Vote`s

* Acts on disputes messages by ..
  * fetching the `PoV` + `Code` blocks
    * running the validation code for the block/candidate

#### Transition Node -> Runtime

* Incoming gossip votes via $mechanics are...
  * stored by sending a message to `VotesDB`
  * passes them on via the proposer/inherents to the runtime

#### Runtime

* incoming message via proposer/inherents are
  * stored them within the runtime (not the state root!)
  * decided upon set of stored votes for the particular dispute
  * decided upon set of stored votes if the dispute is concluded
  * starts the timeout for cleanup of persisted data

### Proposer

#### Node

* Tracks closed disputes
  * transplants them on newly appearing forks without it


* Keep votes around for lt 24 h after block inclusion
Cleanup: whenever

---

## Node

### Gossip / VotesDB / Interaction

TODO: Undefined Component is yet to be discussed with robertk

```mermaid
sequenceDiagram
  participant Net as Network
  participant V as Dispute VotesDB
  participant P as Dispute Participation
  participant PP as Provisioner / Provider
  participant X as Undefined Component
  participant RT as Runtime

  Net ->> P: Receive Gossip Msg
  alt is negative vote
    P ->> V: Store vote
  else is dispute open msg
    loop for each vote
      P ->> V: Store vote
    end
  else is dispute code/pov
    P ->> X: Store Code
  end

  P ->> Net: Gossip vote to interested peers

  opt double vote detected
    P ->> X: Notify other subsystems of double vote of validator
  end

  opt did not cast our vote
    P ->> X: retrieve validation code and PoV
    opt missing PoV or validation code
      P ->> Net: Request them from _one_ randomly picked peer that voted already
    end
    P ->> Ne: Gossip Our Vote to all peers
  end

  P ->> RT: pass vote to runtime via inherent
```

### Proposer / Provisioner

TODO: define interested peers, based on active heads?

TODO: describe transplantation duty of proposer/provisioner

## Runtime

The sequential flow of the runtime logic, on entry.
Note that the storage is fork aware, as such, each fork might have
a different set of votes.

```mermaid
stateDiagram-v2
  [*] --> ExtractInformationFromInherent
  ExtractInformationFromInherent --> StoreVotes
  StoreVotes --> SuperMajorityReached
  SuperMajorityReached --> CreateTransactionWithResolution
  StoreVotes --> [*]
  CreateTransactionWithResolution --> [*]
```

---

Cleanup obsolete votes of concluded disputes on block finalization.

```mermaid
stateDiagram-v2
  [*] --> ObtainSessionIndex
  note left of ObtainSessionIndex: Metric is arbitrary<br /> but must guarantee, the data is kept<br /> for a sufficent amount of time after finalization
  ObtainSessionIndex --> CleanupAllResolvedDisputes
  CleanupAllResolvedDisputes --> [*]
  ObtainSessionIndex --> [*]
```
