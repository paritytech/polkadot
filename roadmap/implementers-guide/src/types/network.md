# Network Types

These types are those that are actually sent over the network to subsystems.

## Universal Types

```rust
type RequestId = u64;
type ProtocolVersion = u32;
struct PeerId(...); // opaque, unique identifier of a peer.
struct View(Vec<Hash>); // Up to `N` (5?) chain heads.

enum ObservedRole {
	Full,
	Light,
}
```

## V1 Network Subsystem Message Types

### Availability Distribution V1

```rust
enum AvailabilityDistributionV1Message {
	/// An erasure chunk for a given candidate hash.
	Chunk(CandidateHash, ErasureChunk),
}
```

### Bitfield Distribution V1

```rust
enum BitfieldDistributionV1Message {
	/// A signed availability bitfield for a given relay-parent hash.
	Bitfield(Hash, SignedAvailabilityBitfield),
}
```

### PoV Distribution V1

```rust
enum PoVDistributionV1Message {
	/// Notification that we are awaiting the given PoVs (by hash) against a
	/// specific relay-parent hash.
	Awaiting(Hash, Vec<Hash>),
	/// Notification of an awaited PoV, in a given relay-parent context.
	/// (relay_parent, pov_hash, pov)
	SendPoV(Hash, Hash, PoV),
}
```

### Statement Distribution V1

```rust
enum StatementDistributionV1Message {
	/// A signed full statement under a given relay-parent.
	Statement(Hash, SignedFullStatement)
}
```

### Collator Protocol V1

```rust
enum CollatorProtocolV1Message {
	/// Declare the intent to advertise collations under a collator ID.
	Declare(CollatorId),
	/// Advertise a collation to a validator. Can only be sent once the peer has declared
	/// that they are a collator with given ID.
	AdvertiseCollation(Hash, ParaId),
	/// Request the advertised collation at that relay-parent.
	RequestCollation(RequestId, Hash, ParaId),
	/// A requested collation.
	Collation(RequestId, CandidateReceipt, PoV),
}
```

## V1 Wire Protocols

### Validation V1

These are the messages for the protocol on the validation peer-set.

```rust
enum ValidationProtocolV1 {
	AvailabilityDistribution(AvailabilityDistributionV1Message),
	BitfieldDistribution(BitfieldDistributionV1Message),
	PoVDistribution(PoVDistributionV1Message),
	StatementDistribution(StatementDistributionV1Message),
}
```

### Collation V1

These are the messages for the protocol on the collation peer-set

```rust
enum CollationProtocolV1 {
	CollatorProtocol(CollatorProtocolV1Message),
}
```

## Network Bridge Event

These updates are posted from the [Network Bridge Subsystem](../node/utility/network-bridge.md) to other subsystems based on registered listeners.

```rust
enum NetworkBridgeEvent<M> {
	/// A peer with given ID is now connected.
	PeerConnected(PeerId, ObservedRole),
	/// A peer with given ID is now disconnected.
	PeerDisconnected(PeerId),
	/// We received a message from the given peer.
	PeerMessage(PeerId, M),
	/// The given peer has updated its description of its view.
	PeerViewChange(PeerId, View), // guaranteed to come after peer connected event.
	/// We have posted the given view update to all connected peers.
	OurViewChange(View),
}
```
