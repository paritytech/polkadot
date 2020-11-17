# Message types

Types of messages that are passed between parachains and the relay chain: UMP, DMP, XCMP.

There is also HRMP (Horizontally Relay-routed Message Passing) which provides the same functionality
although with smaller scalability potential.

## Vertical Message Passing

Types required for message passing between the relay-chain and a parachain.

Actual contents of the messages is specified by the XCM standard.

```rust,ignore
/// A message sent from a parachain to the relay-chain.
type UpwardMessage = Vec<u8>;

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

## Horizontal Message Passing

## HrmpChannelId

A type that uniquely identifies an HRMP channel. An HRMP channel is established between two paras.
In text, we use the notation `(A, B)` to specify a channel between A and B. The channels are
unidirectional, meaning that `(A, B)` and `(B, A)` refer to different channels. The convention is
that we use the first item tuple for the sender and the second for the recipient. Only one channel
is allowed between two participants in one direction, i.e. there cannot be 2 different channels
identified by `(A, B)`.

```rust,ignore
struct HrmpChannelId {
    sender: ParaId,
    recipient: ParaId,
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
