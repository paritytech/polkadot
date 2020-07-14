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

struct UpwardMessage {
	/// The origin for the message to be sent from.
	pub origin: ParachainDispatchOrigin,
	/// The message data.
	pub data: Vec<u8>,
}
```

## Horizontal Message

This is a message sent from a parachain to another parachain that travels through the relay chain.
This message ends up in the recipient's mailbox. A size of a horizontal message is defined by its
`data` payload.

```rust,ignore
struct HorizontalMessage {
	/// The para that will get this message in its downward message queue.
	pub recipient: ParaId,
	/// The message payload.
	pub data: Vec<u8>,
}
```

## Downward Message

A message that go down from the relay chain to a parachain. Such a message could be initiated either
as a result of an operation took place on the relay chain or sent using a horizontal message.

```rust,ignore
enum DownwardMessage {
	/// Some funds were transferred into the parachain's account. The hash is the identifier that
	/// was given with the transfer.
	TransferInto(AccountId, Balance, Remark),
	/// This downward message is a result of a horizontal message represented as opaque bytes sent
	/// by the specified sender.
	HorizontalMessage(ParaId, Vec<u8>),
	/// An opaque message which interpretation is up to the recipient para. This variant ought
	/// to be used as a basis for special protocols between the relay chain and, typically system,
	/// paras.
	ParachainSpecific(Vec<u8>),
}
```
