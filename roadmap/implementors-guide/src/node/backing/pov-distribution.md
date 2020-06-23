# PoV Distribution

This subsystem is responsible for distributing PoV blocks. For now, unified with [Statement Distribution subsystem](statement-distribution.md).

## Protocol

`ProtocolId`: `b"povd"`

Input: [`PoVDistributionMessage`](../../types/overseer-protocol.md#pov-distribution-message)


Output:

- NetworkBridge::RegisterEventProducer(`ProtocolId`)
- NetworkBridge::SendMessage(`[PeerId]`, `ProtocolId`, `Bytes`)
- NetworkBridge::ReportPeer(PeerId, cost_or_benefit)


## Functionality

This network protocol is responsible for distributing [`PoV`s](../../types/availability.md#proof-of-validity) by gossip. Since PoVs are heavy in practice, gossip is far from the most efficient way to distribute them. In the future, this should be replaced by a better network protocol that finds validators who have validated the block and connects to them directly. This protocol is descrbied

This protocol is described in terms of "us" and our peers, with the understanding that this is the procedure that any honest node will run. It has the following goals:
  - We never have to buffer an unbounded amount of data
  - PoVs will flow transitively across a network of honest nodes, stemming from the validators that originally seconded candidates requiring those PoVs.

As we are gossiping, we need to track which PoVs our peers are waiting for to avoid sending them data that they are not expecting. It is not reasonable to expect our peers to buffer unexpected PoVs, just as we will not buffer unexpected PoVs. So notifying our peers about what is being awaited is key. However it is important that the notifications system is also bounded.

For this, in order to avoid reaching into the internals of the [Statement Distribution](statement-distribution.md) Subsystem, we can rely on an expected propery of candidate backing: that each validator can only second one candidate at each chain head. So we can set a cap on the number of PoVs each peer is allowed to notify us that they are waiting for at a given relay-parent. This cap will be the number of validators at that relay-parent. And the view update mechanism of the [Network Bridge](../utility/network-bridge.md) ensures that peers are only allowed to consider a certain set of relay-parents as live. So this bounding mechanism caps the amount of data we need to store per peer at any time at `sum({ n_validators_at_head(head) | head in view_heads })`. Additionally, peers should only be allowed to notify us of PoV hashes they are waiting for in the context of relay-parents in our own local view, which means that `n_validators_at_head` is implied to be `0` for relay-parents not in our own local view.

View updates from peers and our own view updates are received from the network bridge. These will lag somewhat behind the `StartWork` and `StopWork` messages received from the overseer, which will influence the actual data we store. The `OurViewUpdate`s from the [`NetworkBridgeEvent`](../../types/overseer-protocol.md#network-bridge-update) must be considered canonical in terms of our peers' perception of us.

Lastly, the system needs to be bootstrapped with our own perception of which PoVs we are cognizant of but awaiting data for. This is done by receipt of the [`PoVDistributionMessage`](../../types/overseer-protocol.md#pov-distribution-message)::ValidatorStatement variant. We can ignore anything except for `Seconded` statements.

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
	awaited: Set<Hash>, // awaited PoVs by blake2-256 hash.
	fetching: Map<Hash, [ResponseChannel<PoV>]>,
	n_validators: usize,
}

struct PeerState {
	awaited: Map<Hash, Set<Hash>>,
}
```

We also assume the following network messages, which are sent and received by the [Network Bridge](../utility/network-bridge.md)

```rust
enum NetworkMessage {
	/// Notification that we are awaiting the given PoVs (by hash) against a
	/// specific relay-parent hash.
	Awaiting(Hash, Vec<Hash>),
	/// Notification of an awaited PoV, in a given relay-parent context.
	/// (relay_parent, pov_hash, pov)
	SendPoV(Hash, Hash, PoV),
}
```

Here is the logic of the state machine:

*Overseer Signals*
- On `StartWork(relay_parent)`:
	- Get the number of validators at that relay parent by querying the [Runtime API](../utility/runtime-api.md) for the validators and then counting them.
	- Create a blank entry in `relay_parent_state` under `relay_parent` with correct `n_validators` set.
- On `StopWork(relay_parent)`:
	- Remove the entry for `relay_parent` from `relay_parent_state`.
- On `Concluded`: conclude.

*PoV Distribution Messages*
- On `ValidatorStatement(relay_parent, statement)`
	- If this is not `Statement::Seconded`, ignore.
	- If there is an entry under `relay_parent` in `relay_parent_state`, add the `pov_hash` of the seconded Candidate's [`CandidateDescriptor`](../../types/candidate.md#candidate-descriptor) to the `awaited` set of the entry.
	- If the `pov_hash` was not previously awaited and there are `n_validators` or fewer entries in the `awaited` set, send `NetworkMessage::Awaiting(relay_parent, vec![pov_hash])` to all peers.
- On `FetchPoV(relay_parent, descriptor, response_channel)`
	- If there is no entry in `relay_parent_state` under `relay_parent`, ignore.
	- If there is a PoV under `descriptor.pov_hash` in the `known` map, send that PoV on the channel and return.
	- Otherwise, place the `response_channel` in the `fetching` map under `descriptor.pov_hash`.
- On `DistributePoV(relay_parent, descriptor, PoV)`
	- If there is no entry in `relay_parent_state` under `relay_parent`, ignore.
	- Complete and remove any channels under `descriptor.pov_hash` in the `fetching` map.
	- Send `NetworkMessage::SendPoV(relay_parent, descriptor.pov_hash, PoV)` to all peers who have the `descriptor.pov_hash` in the set under `relay_parent` in the `peer.awaited` map and remove the entry from `peer.awaited`.
	- Note the PoV under `descriptor.pov_hash` in `known`.

*Network Bridge Updates*
- On `PeerConnected(peer_id, observed_role)`
	- Make a fresh entry in the `peer_state` map for the `peer_id`.
- On `PeerDisconnected(peer_id)
	- Remove the entry for `peer_id` from the `peer_state` map.
- On `PeerMessage(peer_id, bytes)`
	- If the bytes do not decode to a `NetworkMessage` or the `peer_id` has no entry in the `peer_state` map, report and ignore.
	- If this is `NetworkMessage::Awaiting(relay_parent, pov_hashes)`:
		- If there is no entry under `peer_state.awaited` for the `relay_parent`, report and ignore.
		- If `relay_parent` is not contained within `our_view`, report and ignore.
		- Otherwise, if the `awaited` map combined with the `pov_hashes` would have more than `relay_parent_state[relay_parent].n_validators` entries, report and ignore. Note that we are leaning on the property of the network bridge that it sets our view based on `StartWork` messages.
		- For each new `pov_hash` in `pov_hashes`, if there is a `pov` under `pov_hash` in the `known` map, send the peer a `NetworkMessage::SendPoV(relay_parent, pov_hash, pov)`.
		- Otherwise, add the `pov_hash` to the `awaited` map
	- If this is `NetworkMessage::SendPoV(relay_parent, pov_hash, pov)`:
		- If there is no entry under `relay_parent` in `relay_parent_state` or no entry under `pov_hash` in our `awaited` map for that `relay_parent`, report and ignore.
		- If the blake2-256 hash of the pov doesn't equal `pov_hash`, report and ignore.
		- Complete and remove any listeners in the `fetching` map under `pov_hash`.
		- Add to `known` map.
		- Send `NetworkMessage::SendPoV(relay_parent, descriptor.pov_hash, PoV)` to all peers who have the `descriptor.pov_hash` in the set under `relay_parent` in the `peer.awaited` map and remove the entry from `peer.awaited`.
- On `PeerViewChange(peer_id, view)`
	- If Peer is unknown, ignore.
	- Ensure there is an entry under `relay_parent` for each `relay_parent` in `view` within the `peer.awaited` map, creating blank `awaited` lists as necessary.
	- Remove all entries under `peer.awaited` that are not within `view`.
- On `OurViewChange(view)`
	- Update `our_view` to `view`

