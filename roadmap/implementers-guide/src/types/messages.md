# Message types

Types of messages that are passed between parachains and the relay chain: UMP, DMP, XCMP.

There is also HRMP (Horizontally Relay-routed Message Passing) which provides the same functionality
although with smaller scalability potential.

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
	// Examples:
	// HrmpOpenChannel { .. },
	// HrmpCloseChannel { .. },
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
	pub sent_at: BlockNumber,
	/// The message payload.
	pub data: Vec<u8>,
}
```

## Downward Message

`DownwardMessage`- is a message that goes down from the relay chain to a parachain. Such a message
could be seen as a notification, however, it is conceivable that they might be used by the relay
chain to send a request to the parachain (likely, through the `ParachainSpecific` variant).

```rust,ignore
enum DispatchResult {
	Executed {
		success: bool,
	},
	/// Decoding `RawDispatchable` into an executable runtime representation has failed.
	DecodeFailed,
	/// A dispatchable in question exceeded the maximum amount of weight allowed.
	CriticalWeightExceeded,
}

enum DownwardMessage {
	/// The parachain receives a dispatch result for each sent dispatchable upward message in order
	/// they were sent.
	DispatchResult(Vec<DispatchResult>),
	/// Some funds were transferred into the parachain's account. The hash is the identifier that
	/// was given with the transfer.
	TransferInto(AccountId, Balance, Remark),
	/// An opaque message which interpretation is up to the recipient para. This variant ought
	/// to be used as a basis for special protocols between the relay chain and, typically system,
	/// paras.
	ParachainSpecific(Vec<u8>),
}
```
