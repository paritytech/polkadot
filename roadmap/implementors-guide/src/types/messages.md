# Message types

Types of messages that are passed between parachains and the relay chain: UMP, DMP, XCMP.

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
