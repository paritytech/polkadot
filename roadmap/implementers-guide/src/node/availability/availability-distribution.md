# Availability Distribution

This subsystem is responsible for distribution availability data to peers.
Availability data are chunks, `PoV`s and `AvailableData` (which is `PoV` +
`PersistedValidationData`). It does so via request response protocols.

In particular this subsystem is responsible for:

- Respond to network requests requesting availability data by querying the
  [Availability Store](../utility/availability-store.md).
- Request chunks from backing validators to put them in the local `Availability
  Store` whenever we find an occupied core on any fresh leaf,
  this is to ensure availability by at least 2/3+ of all validators, this
  happens after a candidate is backed.
- Fetch `PoV` from validators, when requested via `FetchPoV` message from
  backing (`pov_requester` module).

The backing subsystem is responsible of making available data available in the
local `Availability Store` upon validation. This subsystem will serve any
network requests by querying that store.

## Protocol

This subsystem does not handle any peer set messages, but the `pov_requester`
does connect to validators of the same backing group on the validation peer
set, to ensure fast propagation of statements between those validators and for
ensuring already established connections for requesting `PoV`s. Other than that
this subsystem drives request/response protocols.

Input:

- `OverseerSignal::ActiveLeaves(ActiveLeavesUpdate)`
- `AvailabilityDistributionMessage{msg: ChunkFetchingRequest}`
- `AvailabilityDistributionMessage{msg: PoVFetchingRequest}`
- `AvailabilityDistributionMessage{msg: FetchPoV}`

Output:

- `NetworkBridgeMessage::SendRequests(Requests, IfDisconnected::TryConnect)`
- `AvailabilityStore::QueryChunk(candidate_hash, index, response_channel)`
- `AvailabilityStore::StoreChunk(candidate_hash, chunk)`
- `AvailabilityStore::QueryAvailableData(candidate_hash, response_channel)`
- `RuntimeApiRequest::SessionIndexForChild`
- `RuntimeApiRequest::SessionInfo`
- `RuntimeApiRequest::AvailabilityCores`

## Functionality

### PoV Requester

The PoV requester in the `pov_requester` module takes care of staying connected
to validators of the current backing group of this very validator on the `Validation`
peer set and it will handle `FetchPoV` requests by issuing network requests to
those validators. It will check the hash of the received `PoV`, but will not do any
further validation. That needs to be done by the original `FetchPoV` sender
(backing subsystem).

### Chunk Requester

After a candidate is backed, the availability of the PoV block must be confirmed
by 2/3+ of all validators. The chunk requester is responsible of making that
availability a reality.

It does that by querying checking occupied cores for all active leaves. For each
occupied core it will spawn a task fetching the erasure chunk which has the
`ValidatorIndex` of the node. For this an `ChunkFetchingRequest` is issued, via
substrate's generic request/response protocol.

The spawned task will start trying to fetch the chunk from validators in
responsible group of the occupied core, in a random order. For ensuring that we
use already open TCP connections wherever possible, the requester maintains a
cache and preserves that random order for the entire session.

Note however that, because not all validators in a group have to be actual
backers, not all of them are required to have the needed chunk. This in turn
could lead to low throughput, as we have to wait for fetches to fail,
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

On the other side the subsystem will listen for incoming `ChunkFetchingRequest`s
and `PoVFetchingRequest`s from the network bridge and will respond to queries,
by looking the requested chunks and `PoV`s up in the availability store, this
happens in the `responder` module.

We rely on the backing subsystem to make available data available locally in the
`Availability Store` after it has validated it.
