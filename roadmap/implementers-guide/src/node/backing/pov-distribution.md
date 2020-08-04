# PoV Distribution

This subsystem is responsible for distributing PoV blocks. For now, unified with [Statement Distribution subsystem](statement-distribution.md).

## Protocol

`PeerSet`: `Validation`

Input: [`PoVDistributionMessage`](../../types/overseer-protocol.md#pov-distribution-message)


Output:

- NetworkBridge::SendMessage(`[PeerId]`, message)
- NetworkBridge::ReportPeer(PeerId, cost_or_benefit)


## Functionality

This network protocol is responsible for distributing [`PoV`s](../../types/availability.md#proof-of-validity) by gossip. Since PoVs are heavy in practice, gossip is far from the most efficient way to distribute them. In the future, this should be replaced by a better network protocol that finds validators who have validated the block and connects to them directly. This protocol is descrbied.

This protocol is described in terms of "us" and our peers, with the understanding that this is the procedure that any honest node will run. It has the following goals:
  - We never have to buffer an unbounded amount of data
  - PoVs will flow transitively across a network of honest nodes, stemming from the validators that originally seconded candidates requiring those PoVs.

As we are gossiping, we need to track which PoVs our peers are waiting for to avoid sending them data that they are not expecting. It is not reasonable to expect our peers to buffer unexpected PoVs, just as we will not buffer unexpected PoVs. So notifying our peers about what is being awaited is key. However it is important that the notifications system is also bounded.

For this, in order to avoid reaching into the internals of the [Statement Distribution](statement-distribution.md) Subsystem, we can rely on an expected propery of candidate backing: that each validator can second up to 2 candidates per chain head. This will typically be only one, because they are only supposed to issue one, but they can equivocate if they are willing to  be slashed. So we can set a cap on the number of PoVs each peer is allowed to notify us that they are waiting for at a given relay-parent. This cap will be twice the number of validators at that relay-parent. In practice, this is a very lax upper bound that can be reduced much further if desired.

The view update mechanism of the [Network Bridge](../utility/network-bridge.md) ensures that peers are only allowed to consider a certain set of relay-parents as live. So this bounding mechanism caps the amount of data we need to store per peer at any time at `sum({ 2 * n_validators_at_head(head) * sizeof(hash) for head in view_heads })`. Additionally, peers should only be allowed to notify us of PoV hashes they are waiting for in the context of relay-parents in our own local view, which means that `n_validators_at_head` is implied to be `0` for relay-parents not in our own local view.

View updates from peers and our own view updates are received from the network bridge. These will lag somewhat behind the `ActiveLeavesUpdate` messages received from the overseer, which will influence the actual data we store. The `OurViewUpdate`s from the [`NetworkBridgeEvent`](../../types/overseer-protocol.md#network-bridge-update) must be considered canonical in terms of our peers' perception of us.

Lastly, the system needs to be bootstrapped with our own perception of which PoVs we are cognizant of but awaiting data for. This is done by receipt of the [`PoVDistributionMessage`](../../types/overseer-protocol.md#pov-distribution-message)::FetchPoV variant. Proper operation of this subsystem depends on the descriptors passed faithfully representing candidates which have been seconded by other validators.

## Formal Description

This protocol can be implemented as a state machine with the following state:

```rust
struct State {
	relay_parent_state: Map<Hash, BlockBasedState>,
	peer_state: Map<PeerId, PeerState>,
	our_view: View,
}

struct BlockBasedState {
	known: Map<Hash, PoV>, // should be a shared PoV in practice. these things are heavy.
	fetching: Map<Hash, [ResponseChannel<PoV>]>,
	n_validators: usize,
}

struct PeerState {
	awaited: Map<Hash, Set<Hash>>,
}
```

We also use the [`PoVDistributionV1Message`](../../types/network.md#pov-distribution) as our `NetworkMessage`, which are sent and received by the [Network Bridge](../utility/network-bridge.md)

Here is the logic of the state machine:

*Overseer Signals*
- On `ActiveLeavesUpdate(relay_parent)`:
	- For each relay-parent in the `activated` list:
		- Get the number of validators at that relay parent by querying the [Runtime API](../utility/runtime-api.md) for the validators and then counting them.
		- Create a blank entry in `relay_parent_state` under `relay_parent` with correct `n_validators` set.
	- For each relay-parent in the `deactivated` list:
		- Remove the entry for `relay_parent` from `relay_parent_state`.
- On `Conclude`: conclude.

*PoV Distribution Messages*
- On `FetchPoV(relay_parent, descriptor, response_channel)`
	- If there is no entry in `relay_parent_state` under `relay_parent`, ignore.
	- If there is a PoV under `descriptor.pov_hash` in the `known` map, send that PoV on the channel and return.
	- Otherwise, place the `response_channel` in the `fetching` map under `descriptor.pov_hash`.
	- If the `pov_hash` had no previous entry in `fetching` and there are `2 * n_validators` or fewer entries in the `fetching` set, send `NetworkMessage::Awaiting(relay_parent, vec![pov_hash])` to all peers.
- On `DistributePoV(relay_parent, descriptor, PoV)`
	- If there is no entry in `relay_parent_state` under `relay_parent`, ignore.
	- Complete and remove any channels under `descriptor.pov_hash` in the `fetching` map.
	- Send `NetworkMessage::SendPoV(relay_parent, descriptor.pov_hash, PoV)` to all peers who have the `descriptor.pov_hash` in the set under `relay_parent` in the `peer.awaited` map and remove the entry from `peer.awaited`.
	- Note the PoV under `descriptor.pov_hash` in `known`.

*Network Bridge Updates*
- On `PeerConnected(peer_id, observed_role)`
	- Make a fresh entry in the `peer_state` map for the `peer_id`.
- On `PeerDisconnected(peer_id)`
	- Remove the entry for `peer_id` from the `peer_state` map.
- On `PeerMessage(peer_id, bytes)`
	- If the bytes do not decode to a `NetworkMessage` or the `peer_id` has no entry in the `peer_state` map, report and ignore.
	- If this is `NetworkMessage::Awaiting(relay_parent, pov_hashes)`:
		- If there is no entry under `peer_state.awaited` for the `relay_parent`, report and ignore.
		- If `relay_parent` is not contained within `our_view`, report and ignore.
		- Otherwise, if the peer's `awaited` map combined with the `pov_hashes` would have more than ` 2 * relay_parent_state[relay_parent].n_validators` entries, report and ignore. Note that we are leaning on the property of the network bridge that it sets our view based on `activated` heads in `ActiveLeavesUpdate` signals.
		- For each new `pov_hash` in `pov_hashes`, if there is a `pov` under `pov_hash` in the `known` map, send the peer a `NetworkMessage::SendPoV(relay_parent, pov_hash, pov)`.
		- Otherwise, add the `pov_hash` to the `awaited` map
	- If this is `NetworkMessage::SendPoV(relay_parent, pov_hash, pov)`:
		- If there is no entry under `relay_parent` in `relay_parent_state` or no entry under `pov_hash` in our `fetching` map for that `relay_parent`, report and ignore.
		- If the blake2-256 hash of the pov doesn't equal `pov_hash`, report and ignore.
		- Complete and remove any listeners in the `fetching` map under `pov_hash`. However, leave an empty set of listeners in the `fetching` map to denote that this was something we once awaited. This will allow us to recognize peers who have sent us something we were expecting, but just a little late.
		- Add to `known` map.
		- Remove the `pov_hash` from the `peer.awaited` map, if any.
		- Send `NetworkMessage::SendPoV(relay_parent, descriptor.pov_hash, PoV)` to all peers who have the `descriptor.pov_hash` in the set under `relay_parent` in the `peer.awaited` map and remove the entry from `peer.awaited`.
- On `PeerViewChange(peer_id, view)`
	- If Peer is unknown, ignore.
	- Ensure there is an entry under `relay_parent` for each `relay_parent` in `view` within the `peer.awaited` map, creating blank `awaited` lists as necessary.
	- Remove all entries under `peer.awaited` that are not within `view`.
	- For all hashes in `view` but were not within the old, send the peer all the keys in our `fetching` map under the block-based state for that hash - i.e. notify the peer of everything we are awaiting at that hash.
- On `OurViewChange(view)`
	- Update `our_view` to `view`

