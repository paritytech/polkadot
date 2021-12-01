# Helpers for Messages Module Integration

The [`messages`](./src/messages.rs) module of this crate contains a bunch of helpers for integrating
messages module into your runtime. Basic prerequisites of these helpers are:
- we're going to bridge Substrate-based chain with another Substrate-based chain;
- both chains have [messages module](../../modules/messages/README.md), Substrate bridge
  module and the [call dispatch module](../../modules/dispatch/README.md);
- all message lanes are identical and may be used to transfer the same messages;
- the messages sent over the bridge are dispatched using
  [call dispatch module](../../modules/dispatch/README.md);
- the messages are `bp_message_dispatch::MessagePayload` structures, where `call` field is
  encoded `Call` of the target chain. This means that the `Call` is opaque to the
  [messages module](../../modules/messages/README.md) instance at the source chain.
  It is pre-encoded by the message submitter;
- all proofs in the [messages module](../../modules/messages/README.md) transactions are
  based on the storage proofs from the bridged chain: storage proof of the outbound message (value
  from the `pallet_bridge_messages::Store::MessagePayload` map), storage proof of the outbound lane
  state (value from the `pallet_bridge_messages::Store::OutboundLanes` map) and storage proof of the
  inbound lane state (value from the `pallet_bridge_messages::Store::InboundLanes` map);
- storage proofs are built at the finalized headers of the corresponding chain. So all message lane
  transactions with proofs are verifying storage proofs against finalized chain headers from
  Substrate bridge module.

**IMPORTANT NOTE**: after reading this document, you may refer to our test runtimes
([rialto_messages.rs](../millau/runtime/src/rialto_messages.rs) and/or
[millau_messages.rs](../rialto/runtime/src/millau_messages.rs)) to see how to use these helpers.

## Contents
- [`MessageBridge` Trait](#messagebridge-trait)
- [`ChainWithMessages` Trait ](#ChainWithMessages-trait)
- [Helpers for the Source Chain](#helpers-for-the-source-chain)
- [Helpers for the Target Chain](#helpers-for-the-target-chain)

## `MessageBridge` Trait

The essence of your integration will be a struct that implements a `MessageBridge` trait. It has
single method (`MessageBridge::bridged_balance_to_this_balance`), used to convert from bridged chain
tokens into this chain tokens. The bridge also requires two associated types to be specified -
`ThisChain` and `BridgedChain`.

Worth to say that if you're going to use hardcoded constant (conversion rate) in the
`MessageBridge::bridged_balance_to_this_balance` method (or in any other method of
`ThisChainWithMessages` or `BridgedChainWithMessages` traits), then you should take a
look at the
[messages parameters functionality](../../modules/messages/README.md#Non-Essential-Functionality).
They allow pallet owner to update constants more frequently than runtime upgrade happens.

## `ChainWithMessages` Trait

The trait is quite simple and can easily be implemented - you just need to specify types used at the
corresponding chain. There is single exception, though (it may be changed in the future):

- `ChainWithMessages::MessagesInstance`: this is used to compute runtime storage keys. There
  may be several instances of messages pallet, included in the Runtime. Every instance stores
  messages and these messages stored under different keys. When we are verifying storage proofs from
  the bridged chain, we should know which instance we're talking to. This is fine, but there's
  significant inconvenience with that - this chain runtime must have the same messages pallet
  instance. This does not necessarily mean that we should use the same instance on both chains -
  this instance may be used to bridge with another chain/instance, or may not be used at all.

## `ThisChainWithMessages` Trait

This trait represents this chain from bridge point of view. Let's review every method of this trait:

- `ThisChainWithMessages::is_outbound_lane_enabled`: is used to check whether given lane accepts
  outbound messages.

- `ThisChainWithMessages::maximal_pending_messages_at_outbound_lane`: you should return maximal
  number of pending (undelivered) messages from this function. Returning small values would require
  relayers to operate faster and could make message sending logic more complicated. On the other
  hand, returning large values could lead to chain state growth.

- `ThisChainWithMessages::estimate_delivery_confirmation_transaction`: you'll need to return
  estimated size and dispatch weight of the delivery confirmation transaction (that happens on
  this chain) from this function.

- `ThisChainWithMessages::transaction_payment`: you'll need to return fee that the submitter
  must pay for given transaction on this chain. Normally, you would use transaction payment pallet
  for this. However, if your chain has non-zero fee multiplier set, this would mean that the
  payment will be computed using current value of this multiplier. But since this transaction
  will be submitted in the future, you may want to choose other value instead. Otherwise,
  non-altruistic relayer may choose not to submit this transaction until number of transactions
  will decrease.

## `BridgedChainWithMessages` Trait

This trait represents this chain from bridge point of view. Let's review every method of this trait:

- `BridgedChainWithMessages::maximal_extrinsic_size`: you will need to return the maximal
  extrinsic size of the target chain from this function.

- `MessageBridge::message_weight_limits`: you'll need to return a range of
  dispatch weights that the outbound message may take at the target chain. Please keep in mind that
  our helpers assume that the message is an encoded call of the target chain. But we never decode
  this call at the source chain. So you can't simply get dispatch weight from pre-dispatch
  information. Instead there are two options to prepare this range: if you know which calls are to
  be sent over your bridge, then you may just return weight ranges for these particular calls.
  Otherwise, if you're going to accept all kinds of calls, you may just return range `[0; maximal
  incoming message dispatch weight]`. If you choose the latter, then you shall remember that the
  delivery transaction itself has some weight, so you can't accept messages with weight equal to
  maximal weight of extrinsic at the target chain. In our test chains, we reject all messages that
  have declared dispatch weight larger than 50% of the maximal bridged extrinsic weight.

- `MessageBridge::estimate_delivery_transaction`: you will need to return estimated dispatch weight and
  size of the delivery transaction that delivers a given message to the target chain. The transaction
  weight must or must not include the weight of pay-dispatch-fee operation, depending on the value
  of `include_pay_dispatch_fee_cost` argument.

- `MessageBridge::transaction_payment`: you'll need to return fee that the submitter
  must pay for given transaction on bridged chain. The best case is when you have the same conversion
  formula on both chains - then you may just reuse the `ThisChainWithMessages::transaction_payment`
  implementation. Otherwise, you'll need to hardcode this formula into your runtime.

## Helpers for the Source Chain

The helpers for the Source Chain reside in the `source` submodule of the
[`messages`](./src/messages.rs) module. The structs are: `FromThisChainMessagePayload`,
`FromBridgedChainMessagesDeliveryProof`, `FromThisChainMessageVerifier`. And the helper functions
are: `maximal_message_size`, `verify_chain_message`, `verify_messages_delivery_proof` and
`estimate_message_dispatch_and_delivery_fee`.

`FromThisChainMessagePayload` is a message that the sender sends through our bridge. It is the
`bp_message_dispatch::MessagePayload`, where `call` field is encoded target chain call. So
at this chain we don't see internals of this call - we just know its size.

`FromThisChainMessageVerifier` is an implementation of `bp_messages::LaneMessageVerifier`. It
has following checks in its `verify_message` method:

1. it'll verify that the used outbound lane is enabled in our runtime;

1. it'll reject messages if there are too many undelivered outbound messages at this lane. The
   sender need to wait while relayers will do their work before sending the message again;

1. it'll reject a message if it has the wrong dispatch origin declared. Like if the submitter is not
   the root of this chain, but it tries to dispatch the message at the target chain using
   `bp_message_dispatch::CallOrigin::SourceRoot` origin. Or he has provided wrong signature
   in the `bp_message_dispatch::CallOrigin::TargetAccount` origin;

1. it'll reject a message if the delivery and dispatch fee that the submitter wants to pay is lesser
   than the fee that is computed using the `estimate_message_dispatch_and_delivery_fee` function.

`estimate_message_dispatch_and_delivery_fee` returns a minimal fee that the submitter needs to pay
for sending a given message. The fee includes: payment for the delivery transaction at the target
chain, payment for delivery confirmation transaction on this chain, payment for `Call` dispatch at
the target chain and relayer interest.

`FromBridgedChainMessagesDeliveryProof` holds the lane identifier and the storage proof of this
inbound lane state at the bridged chain. This also holds the hash of the target chain header, that
was used to generate this storage proof. The proof is verified by the
`verify_messages_delivery_proof`, which simply checks that the target chain header is finalized
(using Substrate bridge module) and then reads the inbound lane state from the proof.

`verify_chain_message` function checks that the message may be delivered to the bridged chain. There
are two main checks:

1. that the message size is less than or equal to the `2/3` of maximal extrinsic size at the target
   chain. We leave `1/3` for signed extras and for the storage proof overhead;

1. that the message dispatch weight is less than or equal to the `1/2` of maximal normal extrinsic
   weight at the target chain. We leave `1/2` for the delivery transaction overhead.

## Helpers for the Target Chain

The helpers for the target chain reside in the `target` submodule of the
[`messages`](./src/messages.rs) module. The structs are: `FromBridgedChainMessagePayload`,
`FromBridgedChainMessagesProof`, `FromBridgedChainMessagesProof`. And the helper functions are:
`maximal_incoming_message_dispatch_weight`, `maximal_incoming_message_size` and
`verify_messages_proof`.

`FromBridgedChainMessagePayload` corresponds to the `FromThisChainMessagePayload` at the bridged
chain. We expect that messages with this payload are stored in the `OutboundMessages` storage map of
the [messages module](../../modules/messages/README.md). This map is used to build
`FromBridgedChainMessagesProof`. The proof holds the lane id, range of message nonces included in
the proof, storage proof of `OutboundMessages` entries and the hash of bridged chain header that has
been used to build the proof. Additionally, there's storage proof may contain the proof of outbound
lane state. It may be required to prune `relayers` entries at this chain (see
[messages module documentation](../../modules/messages/README.md#What-about-other-Constants-in-the-Messages-Module-Configuration-Trait)
for details). This proof is verified by the `verify_messages_proof` function.
