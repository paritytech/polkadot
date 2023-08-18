# Network Types

These types are those that are actually sent over the network to subsystems.

## Universal Types

```rust
type RequestId = u64;
type ProtocolVersion = u32;
struct PeerId(...); // opaque, unique identifier of a peer.
struct View {
	// Up to `N` (5?) chain heads.
	heads: Vec<Hash>,
	// The number of the finalized block.
	finalized_number: BlockNumber,
}

enum ObservedRole {
	Full,
	Light,
}
```

## V1 Network Subsystem Message Types

### Approval Distribution V1

```rust
enum ApprovalDistributionV1Message {
	/// Assignments for candidates in recent, unfinalized blocks.
	///
	/// The u32 is the claimed index of the candidate this assignment corresponds to. Actually checking the assignment
	/// may yield a different result.
	Assignments(Vec<(IndirectAssignmentCert, u32)>),
	/// Approvals for candidates in some recent, unfinalized block.
	Approvals(Vec<IndirectSignedApprovalVote>),
}
```

### Availability Distribution V1

```rust
enum AvailabilityDistributionV1Message {
	/// An erasure chunk for a given candidate hash.
	Chunk(CandidateHash, ErasureChunk),
}
```

### Availability Recovery V1

```rust
enum AvailabilityRecoveryV1Message {
	/// Request a chunk for a given candidate hash and validator index.
	RequestChunk(RequestId, CandidateHash, ValidatorIndex),
	/// Respond with chunk for a given candidate hash and validator index.
	/// The response may be `None` if the requestee does not have the chunk.
	Chunk(RequestId, Option<ErasureChunk>),
	/// Request the full data for a given candidate hash.
	RequestFullData(RequestId, CandidateHash),
	/// Respond with data for a given candidate hash and validator index.
	/// The response may be `None` if the requestee does not have the data.
	FullData(RequestId, Option<AvailableData>),

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
	/// (`relay_parent`, `pov_hash`, `pov`)
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
	/// Declare the intent to advertise collations under a collator ID and `Para`, attaching a
	/// signature of the `PeerId` of the node using the given collator ID key.
	Declare(CollatorId, ParaId, CollatorSignature),
	/// Advertise a collation to a validator. Can only be sent once the peer has
	/// declared that they are a collator with given ID.
	AdvertiseCollation(Hash),
	/// A collation sent to a validator was seconded.
	CollationSeconded(SignedFullStatement),
}
```

## V1 Wire Protocols

### Validation V1

These are the messages for the protocol on the validation peer-set.

```rust
enum ValidationProtocolV1 {
	ApprovalDistribution(ApprovalDistributionV1Message),
	AvailabilityDistribution(AvailabilityDistributionV1Message),
	AvailabilityRecovery(AvailabilityRecoveryV1Message),
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
struct NewGossipTopology {
	/// The session index this topology corresponds to.
	session: SessionIndex,
	/// The topology itself.
	topology: SessionGridTopology,
	/// The local validator index, if any.
	local_index: Option<ValidatorIndex>,
}

struct SessionGridTopology {
	/// An array mapping validator indices to their indices in the
	/// shuffling itself. This has the same size as the number of validators
	/// in the session.
	shuffled_indices: Vec<usize>,
	/// The canonical shuffling of validators for the session.
	canonical_shuffling: Vec<TopologyPeerInfo>,
}

struct TopologyPeerInfo {
	/// The validator's known peer IDs.
	peer_ids: Vec<PeerId>,
	/// The index of the validator in the discovery keys of the corresponding
	/// `SessionInfo`. This can extend _beyond_ the set of active parachain validators.
	validator_index: ValidatorIndex,
	/// The authority discovery public key of the validator in the corresponding
	/// `SessionInfo`.
	discovery_id: AuthorityDiscoveryId,
}

enum NetworkBridgeEvent<M> {
	/// A peer with given ID is now connected.
	PeerConnected(PeerId, ObservedRole, ProtocolVersion, Option<HashSet<AuthorityDiscoveryId>>),
	/// A peer with given ID is now disconnected.
	PeerDisconnected(PeerId),
	/// Our neighbors in the new gossip topology.
	/// We're not necessarily connected to all of them.
	///
	/// This message is issued only on the validation peer set.
	///
	/// Note, that the distribution subsystems need to handle the last
	/// view update of the newly added gossip peers manually.
	NewGossipTopology(NewGossipTopology),
	/// We received a message from the given peer.
	PeerMessage(PeerId, M),
	/// The given peer has updated its description of its view.
	PeerViewChange(PeerId, View), // guaranteed to come after peer connected event.
	/// We have posted the given view update to all connected peers.
	OurViewChange(View),
}
```
