# Helpers for Message Lane Module Integration

The [`messages`](./src/messages.rs) module of this crate contains a bunch of helpers for integrating
message lane module into your runtime. Basic prerequisites of these helpers are:
- we're going to bridge Substrate-based chain with another Substrate-based chain;
- both chains have [message lane module](../../modules/message-lane/README.md), Substrate bridge
  module and the [call dispatch module](../../modules/call-dispatch/README.md);
- all message lanes are identical and may be used to transfer the same messages;
- the messages sent over the bridge are dispatched using
  [call dispatch module](../../modules/call-dispatch/README.md);
- the messages are `pallet_bridge_call_dispatch::MessagePayload` structures, where `call` field is
  encoded `Call` of the target chain. This means that the `Call` is opaque to the
  [message lane module](../../modules/message-lane/README.md) instance at the source chain.
  It is pre-encoded by the message submitter;
- all proofs in the [message lane module](../../modules/message-lane/README.md) transactions are
  based on the storage proofs from the bridged chain: storage proof of the outbound message (value
  from the `pallet_message_lane::Store::MessagePayload` map), storage proof of the outbound lane
  state (value from the `pallet_message_lane::Store::OutboundLanes` map) and storage proof of the
  inbound lane state (value from the `pallet_message_lane::Store::InboundLanes` map);
- storage proofs are built at the finalized headers of the corresponding chain. So all message lane
  transactions with proofs are verifying storage proofs against finalized chain headers from
  Substrate bridge module.

**IMPORTANT NOTE**: after reading this document, you may refer to our test runtimes
([rialto_messages.rs](../millau/runtime/src/rialto_messages.rs) and/or
[millau_messages.rs](../rialto/runtime/src/millau_messages.rs)) to see how to use these helpers.

## Contents
- [`MessageBridge` Trait](#messagebridge-trait)
- [`ChainWithMessageLanes` Trait ](#chainwithmessagelanes-trait)
- [Helpers for the Source Chain](#helpers-for-the-source-chain)
- [Helpers for the Target Chain](#helpers-for-the-target-chain)

## `MessageBridge` Trait

The essence of your integration will be a struct that implements a `MessageBridge` trait. Let's
review every method and give some implementation hints here:

- `MessageBridge::maximal_extrinsic_size_on_target_chain`: you will need to return the maximal
  extrinsic size of the target chain from this function. This may be the constant that is updated
  when your runtime is upgraded, or you may use the
  [message lane parameters functionality](../../modules/message-lane/README.md#Non-Essential-Functionality)
  to allow the pallet owner to update this value more frequently (you may also want to use this
  functionality for all constants that are used in other methods described below).

- `MessageBridge::weight_limits_of_message_on_bridged_chain`: you'll need to return a range of
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

- `MessageBridge::weight_of_delivery_transaction`: you will need to return the maximal weight of the
  delivery transaction that delivers a given message to the target chain. There are three main
  things to notice:

  1. weight, returned from this function is then used to compute the fee that the
  message sender needs to pay for the delivery transaction. So it shall not be a simple dispatch
  weight of delivery call - it should be the "weight" of the transaction itself, including per-byte
  "weight", "weight" of signed extras and etc.
  1. the delivery transaction brings storage proof of
  the message, not the message itself. So your transaction will include extra bytes. We suggest
  computing the size of single empty value storage proof at the source chain, increase this value a
  bit and hardcode it in the source chain runtime code. This size then must be added to the size of
  payload and included in the weight computation;
  1. before implementing this function, please take
  a look at the
  [weight formula of delivery transaction](../../modules/message-lane/README.md#Weight-of-receive_messages_proof-call).
  It adds some extra weight for every additional byte of the proof (everything above
  `pallet_message_lane::EXPECTED_DEFAULT_MESSAGE_LENGTH`), so it's not trivial. Even better, please
  refer to [our implementation](../millau/runtime/src/rialto_messages.rs) for test chains for
  details.

- `MessageBridge::weight_of_delivery_confirmation_transaction_on_this_chain`: you'll need to return
  the maximal weight of a single message delivery confirmation transaction on this chain. All points
  from the previous paragraph are also relevant here.

- `MessageBridge::this_weight_to_this_balance`: this function needs to convert weight units into fee
  units on this chain. Most probably this can be done by calling
  `pallet_transaction_payment::Config::WeightToFee::calc()` for passed weight.

- `MessageBridge::bridged_weight_to_bridged_balance`: this function needs to convert weight units
  into fee units on the target chain. The best case is when you have the same conversion formula on
  both chains - then you may just call the same formula from the previous paragraph. Otherwise,
  you'll need to hardcode this formula into your runtime.

- `MessageBridge::bridged_balance_to_this_balance`: this may be the easiest method to implement and
  the hardest to maintain at the same time. If you don't have any automatic methods to determine
  conversion rate, then you'll probably need to maintain it by yourself (by updating conversion
  rate, stored in runtime storage). This means that if you're too late with an update, then you risk
  to accept messages with lower-than-expected fee. So it may be wise to have some reserve in this
  conversion rate, even if that means larger delivery and dispatch fees.

## `ChainWithMessageLanes` Trait

Apart from its methods, `MessageBridge` also has two associated types that are implementing the
`ChainWithMessageLanes` trait. One is for this chain and the other is for the bridged chain. The
trait is quite simple and can easily be implemented - you just need to specify types used at the
corresponding chain. There are two exceptions, though. Both may be changed in the future. Here they
are:

- `ChainWithMessageLanes::Call`: it isn't a good idea to reference bridged chain runtime from your
  runtime (cyclic references + maintaining on upgrades). So you can't know the type of bridged chain
  call in your runtime. This type isn't actually used at this chain, so you may use `()` instead.

- `ChainWithMessageLanes::MessageLaneInstance`: this is used to compute runtime storage keys. There
  may be several instances of message lane pallet, included in the Runtime. Every instance stores
  messages and these messages stored under different keys. When we are verifying storage proofs from
  the bridged chain, we should know which instance we're talking to. This is fine, but there's
  significant inconvenience with that - this chain runtime must have the same message lane pallet
  instance. This does not necessarily mean that we should use the same instance on both chains -
  this instance may be used to bridge with another chain/instance, or may not be used at all.

## Helpers for the Source Chain

The helpers for the Source Chain reside in the `source` submodule of the
[`messages`](./src/messages.rs) module. The structs are: `FromThisChainMessagePayload`,
`FromBridgedChainMessagesDeliveryProof`, `FromThisChainMessageVerifier`. And the helper functions
are: `maximal_message_size`, `verify_chain_message`, `verify_messages_delivery_proof` and
`estimate_message_dispatch_and_delivery_fee`.

`FromThisChainMessagePayload` is a message that the sender sends through our bridge. It is the
`pallet_bridge_call_dispatch::MessagePayload`, where `call` field is encoded target chain call. So
at this chain we don't see internals of this call - we just know its size.

`FromThisChainMessageVerifier` is an implementation of `bp_message_lane::LaneMessageVerifier`. It
has following checks in its `verify_message` method:

1. it'll verify that the used outbound lane is enabled in our runtime;

1. it'll reject messages if there are too many undelivered outbound messages at this lane. The
   sender need to wait while relayers will do their work before sending the message again;

1. it'll reject a message if it has the wrong dispatch origin declared. Like if the submitter is not
   the root of this chain, but it tries to dispatch the message at the target chain using
   `pallet_bridge_call_dispatch::CallOrigin::SourceRoot` origin. Or he has provided wrong signature
   in the `pallet_bridge_call_dispatch::CallOrigin::TargetAccount` origin;

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
the [message lane module](../../modules/message-lane/README.md). This map is used to build
`FromBridgedChainMessagesProof`. The proof holds the lane id, range of message nonces included in
the proof, storage proof of `OutboundMessages` entries and the hash of bridged chain header that has
been used to build the proof. Additionally, there's storage proof may contain the proof of outbound
lane state. It may be required to prune `relayers` entries at this chain (see
[message lane module documentation](../../modules/message-lane/README.md#What-about-other-Constants-in-the-Message-Lane-Module-Configuration-Trait)
for details). This proof is verified by the `verify_messages_proof` function.
