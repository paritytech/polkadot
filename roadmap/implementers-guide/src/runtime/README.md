# Runtime Architecture

It's clear that we want to separate different aspects of the runtime logic into different modules. Modules define their own storage, routines, and entry-points. They also define initialization and finalization logic.

Due to the (lack of) guarantees provided by a particular blockchain-runtime framework, there is no defined or dependable order in which modules' initialization or finalization logic will run. Supporting this blockchain-runtime framework is important enough to include that same uncertainty in our model of runtime modules in this guide. Furthermore, initialization logic of modules can trigger the entry-points or routines of other modules. This is one architectural pressure against dividing the runtime logic into multiple modules. However, in this case the benefits of splitting things up outweigh the costs, provided that we take certain precautions against initialization and entry-point races.

We also expect, although it's beyond the scope of this guide, that these runtime modules will exist alongside various other modules. This has two facets to consider. First, even if the modules that we describe here don't invoke each others' entry points or routines during initialization, we still have to protect against those other modules doing that. Second, some of those modules are expected to provide governance capabilities for the chain. Configuration exposed by parachain-host modules is mostly for the benefit of these governance modules, to allow the operators or community of the chain to tweak parameters.

The runtime's primary roles to manage scheduling and updating of parachains and parathreads, as well as handling misbehavior reports and slashing. This guide doesn't focus on how parachains or parathreads are registered, only that they are. Also, this runtime description assumes that validator sets are selected somehow, but doesn't assume any other details than a periodic _session change_ event. Session changes give information about the incoming validator set and the validator set of the following session.

The runtime also serves another role, which is to make data available to the Node-side logic via Runtime APIs. These Runtime APIs should be sufficient for the Node-side code to author blocks correctly.

There is some functionality of the relay chain relating to parachains that we also consider beyond the scope of this document. In particular, all modules related to how parachains are registered aren't part of this guide, although we do provide routines that should be called by the registration process.

We will split the logic of the runtime up into these modules:

* Initializer: manage initialization order of the other modules.
* Shared: manages shared storage and configurations for other modules.
* Configuration: manage configuration and configuration updates in a non-racy manner.
* Paras: manage chain-head and validation code for parachains and parathreads.
* Scheduler: manages parachain and parathread scheduling as well as validator assignments.
* Inclusion: handles the inclusion and availability of scheduled parachains and parathreads.
* Validity: handles secondary checks and dispute resolution for included, available parablocks.
* Hrmp: handles horizontal messages between paras.
* Ump: Handles upward messages from a para to the relay chain.
* Dmp: Handles downward messages from the relay chain to the para.

The [Initializer module](initializer.md) is special - it's responsible for handling the initialization logic of the other modules to ensure that the correct initialization order and related invariants are maintained. The other modules won't specify a on-initialize logic, but will instead expose a special semi-private routine that the initialization module will call. The other modules are relatively straightforward and perform the roles described above.

The Parachain Host operates under a changing set of validators. Time is split up into periodic sessions, where each session brings a potentially new set of validators. Sessions are buffered by one, meaning that the validators of the upcoming session `n+1` are determined at the end of session `n-1`, right before session `n` starts. Parachain Host runtime modules need to react to changes in the validator set, as it will affect the runtime logic for processing candidate backing, availability bitfields, and misbehavior reports. The Parachain Host modules can't determine ahead-of-time exactly when session change notifications are going to happen within the block (note: this depends on module initialization order again - better to put session before parachains modules).

The relay chain is intended to use BABE or SASSAFRAS, which both have the property that a session changing at a block is determined not by the number of the block but instead by the time the block is authored. In some sense, sessions change in-between blocks, not at blocks. This has the side effect that the session of a child block cannot be determined solely by the parent block's identifier. Being able to unilaterally determine the validator-set at a specific block based on its parent hash would make a lot of Node-side logic much simpler.

In order to regain the property that the validator set of a block is predictable by its parent block, we delay session changes' application to Parachains by 1 block. This means that if there is a session change at block X, that session change will be stored and applied during initialization of direct descendents of X. This principal side effect of this change is that the Parachains runtime can disagree with session or consensus modules about which session it currently is. Misbehavior reporting routines in particular will be affected by this, although not severely. The parachains runtime might believe it is the last block of the session while the system is really in the first block of the next session. In such cases, a historical validator-set membership proof will need to accompany any misbehavior report, although they typically do not need to during current-session misbehavior reports.

So the other role of the initializer module is to forward session change notifications to modules in the initialization order. Session change is also the point at which the [Configuration Module](configuration.md) updates the configuration. Most of the other modules will handle changes in the configuration during their session change operation, so the initializer should provide both the old and new configuration to all the other
modules alongside the session change notification. This means that a session change notification should consist of the following data:

```rust
struct SessionChangeNotification {
 // The new validators in the session.
 validators: Vec<ValidatorId>,
 // The validators for the next session.
 queued: Vec<ValidatorId>,
 // The configuration before handling the session change.
 prev_config: HostConfiguration,
 // The configuration after handling the session change.
 new_config: HostConfiguration,
 // A secure randomn seed for the session, gathered from BABE.
 random_seed: [u8; 32],
 // The session index of the beginning session.
 session_index: SessionIndex,
}
```

> TODO Diagram: order of runtime operations (initialization, session change)
