# Runtime Architecture

It's clear that we want to separate different aspects of the runtime logic into different modules. Modules define their own storage, routines, and entry-points. They also define initialization and finalization logic.

Due to the (lack of) guarantees provided by a particular blockchain-runtime framework, there is no defined or dependable order in which modules' initialization or finalization logic will run. Supporting this blockchain-runtime framework is important enough to include that same uncertainty in our model of runtime modules in this guide. Furthermore, initialization logic of modules can trigger the entry-points or routines of other modules. This is one architectural pressure against dividing the runtime logic into multiple modules. However, in this case the benefits of splitting things up outweigh the costs, provided that we take certain precautions against initialization and entry-point races.

We also expect, although it's beyond the scope of this guide, that these runtime modules will exist alongside various other modules. This has two facets to consider. First, even if the modules that we describe here don't invoke each others' entry points or routines during initialization, we still have to protect against those other modules doing that. Second, some of those modules are expected to provide governance capabilities for the chain. Configuration exposed by parachain-host modules is mostly for the benefit of these governance modules, to allow the operators or community of the chain to tweak parameters.

The runtime's primary roles to manage scheduling and updating of parachains and parathreads, as well as handling misbehavior reports and slashing. This guide doesn't focus on how parachains or parathreads are registered, only that they are. Also, this runtime description assumes that validator sets are selected somehow, but doesn't assume any other details than a periodic _session change_ event. Session changes give information about the incoming validator set and the validator set of the following session.

The runtime also serves another role, which is to make data available to the Node-side logic via Runtime APIs. These Runtime APIs should be sufficient for the Node-side code to author blocks correctly.

There is some functionality of the relay chain relating to parachains that we also consider beyond the scope of this document. In particular, all modules related to how parachains are registered aren't part of this guide, although we do provide routines that should be called by the registration process.

We will split the logic of the runtime up into these modules:

* Initializer: manage initialization order of the other modules.
* Configuration: manage configuration and configuration updates in a non-racy manner.
* Paras: manage chain-head and validation code for parachains and parathreads.
* Scheduler: manages parachain and parathread scheduling as well as validator assignments.
* Inclusion: handles the inclusion and availability of scheduled parachains and parathreads.
* Validity: handles secondary checks and dispute resolution for included, available parablocks.

The [Initializer module](initializer.html) is special - it's responsible for handling the initialization logic of the other modules to ensure that the correct initialization order and related invariants are maintained. The other modules won't specify a on-initialize logic, but will instead expose a special semi-private routine that the initialization module will call. The other modules are relatively straightforward and perform the roles described above.

The Parachain Host operates under a changing set of validators. Time is split up into periodic sessions, where each session brings a potentially new set of validators. Sessions are buffered by one, meaning that the validators of the upcoming session are fixed and always known. Parachain Host runtime modules need to react to changes in the validator set, as it will affect the runtime logic for processing candidate backing, availability bitfields, and misbehavior reports. The Parachain Host modules can't determine ahead-of-time exactly when session change notifications are going to happen within the block (note: this depends on module initialization order again - better to put session before parachains modules). Ideally, session changes are always handled before initialization. It is clearly a problem if we compute validator assignments to parachains during initialization and then the set of validators changes. In the best case, we can recognize that re-initialization needs to be done. In the worst case, bugs would occur.

There are 3 main ways that we can handle this issue:

1. Establish an invariant that session change notifications always happen after initialization. This means that when we receive a session change notification before initialization, we call the initialization routines before handling the session change.
1. Require that session change notifications always occur before initialization. Brick the chain if session change notifications ever happen after initialization.
1. Handle both the before and after cases.

Although option 3 is the most comprehensive, it runs counter to our goal of simplicity. Option 1 means requiring the runtime to do redundant work at all sessions and will also mean, like option 3, that designing things in such a way that initialization can be rolled back and reapplied under the new environment. That leaves option 2, although it is a "nuclear" option in a way and requires us to constrain the parachain host to only run in full runtimes with a certain order of operations.

So the other role of the initializer module is to forward session change notifications to modules in the initialization order, throwing an unrecoverable error if the notification is received after initialization. Session change is the point at which the [Configuration Module](configuration.html) updates the configuration. Most of the other modules will handle changes in the configuration during their session change operation, so the initializer should provide both the old and new configuration to all the other
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
}
```

> REVIEW: other options? arguments in favor of going for options 1 or 3 instead of 2. we could do a "soft" version of 2 where we note that the chain is potentially broken due to bad initialization order
> TODO Diagram: order of runtime operations (initialization, session change)
