# `ParaInherent`

This module is responsible for providing all data given to the runtime by the block author to the various parachains modules. The entry-point is mandatory, in that it must be invoked exactly once within every block, and it is also "inherent", in that it is provided with no origin by the block author. The data within it carries its own authentication; i.e. the data takes the form of signed statements by validators. Invalid data will be filtered and not applied.

This module does not have the same initialization/finalization concerns as the others, as it only requires that entry points be triggered after all modules have initialized and that finalization happens after entry points are triggered. Both of these are assumptions we have already made about the runtime's order of operations, so this module doesn't need to be initialized or finalized by the `Initializer`.

There are a couple of important notes to the operations in this inherent as they relate to disputes.

1. We don't accept bitfields or backed candidates if in "governance-only" mode from having a local dispute conclude on this fork.
1. When disputes are initiated, we remove the block from pending availability. This allows us to roll back chains to the block before blocks are included as opposed to backing. It's important to do this before processing bitfields.
1. `Inclusion::collect_disputed` is kind of expensive so it's important to gate this on whether there are actually any new disputes. Which should be never.
1. And we don't accept parablocks that have open disputes or disputes that have concluded against the candidate. It's important to import dispute statements before backing, but this is already the case as disputes are imported before processing bitfields.

## Storage

```rust
/// Whether the para inherent was included or not.
Included: Option<()>,
```

```rust
/// Scraped on chain votes to be used in disputes off-chain.
OnChainVotes: Option<ScrapedOnChainVotes>,
```

## Finalization

1. Take (get and clear) the value of `Included`. If it is not `Some`, throw an unrecoverable error.

## Entry Points

* `enter`: This entry-point accepts one parameter: [`ParaInherentData`](../types/runtime.md#ParaInherentData).
* `create_inherent`: This entry-point accepts one parameter: `InherentData`.

Both entry points share mostly the same code. `create_inherent` will
meaningfully limit inherent data to adhere to the weight limit, in addition to
sanitizing any inputs and filtering out invalid data. Conceptually it is part of
the block production. The `enter` call on the other hand is part of block import
and consumes/imports the data previously produced by `create_inherent`.

In practice both calls process inherent data and apply it to the state. Block
production and block import should arrive at the same new state. Hence we re-use
the same logic to ensure this is the case.

The only real difference between the two is, that on `create_inherent` we
actually need the processed and filtered inherent data to build the block, while
on `enter` the processed data should for one be identical to the incoming
inherent data (assuming honest block producers) and second it is irrelevant, as
we are not building a block but just processing it, so the processed inherent
data is simply dropped.

This also means that the `enter` function keeps data around for no good reason.
This seems acceptable though as the size of a block is rather limited.
Nevertheless if we ever wanted to optimize this we can easily implement an
inherent collector that has two implementations, where one clones and stores the
data and the other just passes it on.

