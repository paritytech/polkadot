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

#### Node

##### Gossip / VotesDB / Interaction

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

  P ->> RT: pass the new votes to runtime via $mechanics
```

TODO: what is $mechanics
TODO: define interested peers, based on active heads?


##### Enhancement of Existing subsystems

Now the proposer and provisioner need to transplant
the resolved disputes to all upcoming forks.
###### Provisioner

Since the proposer must track all inclusion inherents, up to the point where the block is finalized + time ε, at which point
there will be no more forks that will need this dispute resolution.

`InclusionInherent` hence is the mean of transplantation within the subsystem.

```mermaid
stateDiagram-v2
  [*] --> InherentsContainsDisputeResolution
  [*] --> CheckIfStoredResolution
  CheckIfStoredResolution --> AddDisputeResolutionInherentData: yes
  CheckIfStoredResolution --> [*]: no
  InherentsContainsDisputeResolution --> AddDisputeResolutionInherentData
  AddDisputeResolutionInherentData --> [*]
```

```mermaid
stateDiagram-v2
  [*] --> InherentsContainsDisputeResolution
  SaveResolution --> [*]
```

```mermaid
stateDiagram-v2
  [*] --> BlockFinalization
  BlockFinalization --> StartTimeout
  StartTimeout --> CleanupResolutionInherent
  CleanupStoredResolution --> [*]
```

##### Proposer

The proposer only needs to include the additional inherent
data as received from the `provisioner`, which it then includes in the proposed block.

#### Runtime

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
