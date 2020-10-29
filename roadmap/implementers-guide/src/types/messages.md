# Message types

Types of messages that are passed between parachains and the relay chain: UMP, DMP, XCMP.

There is also HRMP (Horizontally Relay-routed Message Passing) which provides the same functionality
although with smaller scalability potential.

## Vertical Message Passing

Types required for message passing between the relay-chain and a parachain.

Actual contents of the messages is specified by the XCM standard.

```rust,ignore
/// A message sent from the relay-chain down to a parachain.
///
/// The size of the message is limited by the `config.max_downward_message_size`
/// parameter.
type DownwardMessage = Vec<u8>;

/// This struct extends `DownwardMessage` by adding the relay-chain block number when the message was
/// enqueued in the downward message queue.
struct InboundDownwardMessage {
	/// The block number at which this messages was put into the downward message queue.
	pub sent_at: BlockNumber,
	/// The actual downward message to processes.
	pub msg: DownwardMessage,
}
```

## HrmpChannelId

A type that uniquely identifies an HRMP channel. An HRMP channel is established between two paras.
In text, we use the notation `(A, B)` to specify a channel between A and B. The channels are
unidirectional, meaning that `(A, B)` and `(B, A)` refer to different channels. The convention is
that we use the first item tuple for the sender and the second for the recipient. Only one channel
is allowed between two participants in one direction, i.e. there cannot be 2 different channels
identified by `(A, B)`.

`HrmpChannelId` has a defined ordering: first `sender` and tie is resolved by `recipient`.

```rust,ignore
struct HrmpChannelId {
    sender: ParaId,
    recipient: ParaId,
}
```

## Upward Message

A type of messages dispatched from a parachain to the relay chain.

```rust,ignore
enum ParachainDispatchOrigin {
	/// As a simple `Origin::Signed`, using `ParaId::account_id` as its value. This is good when
	/// interacting with standard modules such as `balances`.
	Signed,
	/// As the special `Origin::Parachain(ParaId)`. This is good when interacting with parachain-
	/// aware modules which need to succinctly verify that the origin is a parachain.
	Parachain,
	/// As the simple, superuser `Origin::Root`. This can only be done on specially permissioned
	/// parachains.
	Root,
}

/// An opaque byte buffer that encodes an entrypoint and the arguments that should be
/// provided to it upon the dispatch.
///
/// NOTE In order to be executable the byte buffer should be decoded which potentially can fail if
/// the encoding was changed.
type RawDispatchable = Vec<u8>;

enum UpwardMessage {
	/// This upward message is meant to schedule execution of a provided dispatchable.
	Dispatchable {
		/// The origin with which the dispatchable should be executed.
		origin: ParachainDispatchOrigin,
		/// The dispatchable to be executed in its raw form.
		dispatchable: RawDispatchable,
	},
	/// A message for initiation of opening a new HRMP channel between the origin para and the
	/// given `recipient`.
	///
	/// Let `origin` be the parachain that sent this upward message. In that case the channel
	/// to be opened is (`origin` -> `recipient`).
	HrmpInitOpenChannel {
		/// The receiving party in the channel.
		recipient: ParaId,
		/// How many messages can be stored in the channel at most.
		max_places: u32,
		/// The maximum size of a message in this channel.
		max_message_size: u32,
	},
	/// A message that is meant to confirm the HRMP open channel request initiated earlier by the
	/// `HrmpInitOpenChannel` by the given `sender`.
	///
	/// Let `origin` be the parachain that sent this upward message. In that case the channel
	/// (`origin` -> `sender`) will be opened during the session change.
	HrmpAcceptOpenChannel(ParaId),
	/// A message for closing the specified existing channel `ch`.
	///
	/// The channel to be closed is `(ch.sender -> ch.recipient)`. The parachain that sent this
	/// upward message must be either `ch.sender` or `ch.recipient`.
	HrmpCloseChannel(HrmpChannelId),
}
```

## Horizontal Message

This is a message sent from a parachain to another parachain that travels through the relay chain.
This message ends up in the recipient's mailbox. A size of a horizontal message is defined by its
`data` payload.

```rust,ignore
struct OutboundHrmpMessage {
	/// The para that will get this message in its downward message queue.
	pub recipient: ParaId,
	/// The message payload.
	pub data: Vec<u8>,
}

struct InboundHrmpMessage {
	/// The block number at which this message was sent.
	/// Specifically, it is the block number at which the candidate that sends this message was
	/// enacted.
	pub sent_at: BlockNumber,
	/// The message payload.
	pub data: Vec<u8>,
}
```
