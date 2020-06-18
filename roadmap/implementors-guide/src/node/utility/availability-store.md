# Availability Store

This is a utility subsystem responsible for keeping available certain data and pruning that data.

The two data types:

- Full PoV blocks of candidates we have validated
- Availability chunks of candidates that were backed and noted available on-chain.

For each of these data we have pruning rules that determine how long we need to keep that data available.

PoV hypothetically only need to be kept around until the block where the data was made fully available is finalized. However, disputes can revert finality, so we need to be a bit more conservative. We should keep the PoV until a block that finalized availability of it has been finalized for 1 day.

> TODO: arbitrary, but extracting `acceptance_period` is kind of hard here...

Availability chunks need to be kept available until the dispute period for the corresponding candidate has ended. We can accomplish this by using the same criterion as the above, plus a delay. This gives us a pruning condition of the block finalizing availability of the chunk being final for 1 day + 1 hour.

> TODO: again, concrete acceptance-period would be nicer here, but complicates things

There is also the case where a validator commits to make a PoV available, but the corresponding candidate is never backed. In this case, we keep the PoV available for 1 hour.

> TODO: ideally would be an upper bound on how far back contextual execution is OK.

There may be multiple competing blocks all ending the availability phase for a particular candidate. Until (and slightly beyond) finality, it will be unclear which of those is actually the canonical chain, so the pruning records for PoVs and Availability chunks should keep track of all such blocks.

## Protocol

Input: [`AvailabilityStoreMessage`](../../types/overseer-protocol.html#availability-store-message)

## Functionality

On `StartWork`:

- Note any new candidates backed in the block. Update pruning records for any stored `PoVBlock`s.
- Note any newly-included candidates backed in the block. Update pruning records for any stored availability chunks.

On block finality events:

- > TODO: figure out how we get block finality events from overseer
- Handle all pruning based on the newly-finalized block.

On `QueryPoV` message:

- Return the PoV block, if any, for that candidate hash.

On `QueryChunk` message:

- Determine if we have the chunk indicated by the parameters and return it and its inclusion proof via the response channel if so.

On `StoreChunk` message:

- Store the chunk along with its inclusion proof under the candidate hash and validator index.
