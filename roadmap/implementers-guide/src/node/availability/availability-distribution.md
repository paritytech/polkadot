# Availability Distribution

Distribute availability erasure-coded chunks to validators.

After a candidate is backed, the availability of the PoV block must be confirmed
by 2/3+ of all validators. Backing nodes will serve chunks for a PoV block from
their [Availability Store](../utility/availability-store.md), all other
validators request their chunks from backing nodes and store those received chunks in
their local availability store.

## Protocol

This subsystem has no associated peer set right now, but instead relies on
a request/response protocol, defined by `Protocol::AvailabilityFetching`.

Input:

- OverseerSignal::ActiveLeaves(`[ActiveLeavesUpdate]`)
- AvailabilityDistributionMessage{msg: AvailabilityFetchingRequest}

Output:

- NetworkBridgeMessage::SendRequests(`[Requests]`)
- AvailabilityStore::QueryChunk(candidate_hash, index, response_channel)
- AvailabilityStore::StoreChunk(candidate_hash, chunk)
- RuntimeApiRequest::SessionIndexForChild
- RuntimeApiRequest::SessionInfo
- RuntimeApiRequest::AvailabilityCores

## Functionality

### Requesting

This subsystems monitors currently occupied cores for all active leaves. For
each occupied core it will spawn a task fetching the erasure chunk which has the
`ValidatorIndex` of the node. For this an `AvailabilityFetchingRequest` is
issued, via substrate's generic request/response protocol.

The spawned task will start trying to fetch the chunk from validators in
responsible group of the occupied core, in a random order. For ensuring that we
use already open TCP connections wherever possible, the subsystem maintains a
cache and preserves that random order for the entire session.

Note however that, because not all validators in a group have to be actual
backers, not all of them are required to have the needed chunk. This in turn
could lead to low throughput, as we have to wait for a fetches to fail,
before reaching a validator finally having our chunk. We do rank back validators
not delivering our chunk, but as backers could vary from block to block on a
perfectly legitimate basis, this is still not ideal. See issues [2509](https://github.com/paritytech/polkadot/issues/2509) and [2512](https://github.com/paritytech/polkadot/issues/2512)
for more information.

The current implementation also only fetches chunks for occupied cores in blocks
in active leaves. This means though, if active leaves skips a block or we are
particularly slow in fetching our chunk, we might not fetch our chunk if
availability reached 2/3 fast enough (slot becomes free). This is not desirable
as we would like as many validators as possible to have their chunk. See this
[issue](https://github.com/paritytech/polkadot/issues/2513) for more details.


### Serving

On the other side the subsystem will listen for incoming
`AvailabilityFetchingRequest`s from the network bridge and will respond to
queries, by looking the requested chunk up in the availability store.
