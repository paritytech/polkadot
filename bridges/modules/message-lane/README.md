# Message Lane Module

The message lane module is used to deliver messages from source chain to target chain. Message is
(almost) opaque to the module and the final goal is to hand message to the message dispatch
mechanism.

## Contents
- [Overview](#overview)
- [Message Workflow](#message-workflow)
- [Integrating Message Lane Module into Runtime](#integrating-message-lane-module-into-runtime)
- [Non-Essential Functionality](#non-essential-functionality)
- [Weights of Module Extrinsics](#weights-of-module-extrinsics)

## Overview

Message lane is an unidirectional channel, where messages are sent from source chain to the target
chain. At the same time, a single instance of message lane module supports both outbound lanes and
inbound lanes. So the chain where the module is deployed (this chain), may act as a source chain for
outbound messages (heading to a bridged chain) and as a target chain for inbound messages (coming
from a bridged chain).

Message lane module supports multiple message lanes. Every message lane is identified with a 4-byte
identifier. Messages sent through the lane are assigned unique (for this lane) increasing integer
value that is known as nonce ("number that can only be used once"). Messages that are sent over the
same lane are guaranteed to be delivered to the target chain in the same order they're sent from
the source chain. In other words, message with nonce `N` will be delivered right before delivering a
message with nonce `N+1`.

Single message lane may be seen as a transport channel for single application (onchain, offchain or
mixed). At the same time the module itself never dictates any lane or message rules. In the end, it
is the runtime developer who defines what message lane and message mean for this runtime.

## Message Workflow

The message "appears" when its submitter calls the `send_message()` function of the module. The
submitter specifies the lane that he's willing to use, the message itself and the fee that he's
willing to pay for the message delivery and dispatch. If a message passes all checks, the nonce is
assigned and the message is stored in the module storage. The message is in an "undelivered" state
now.

We assume that there are external, offchain actors, called relayers, that are submitting module
related transactions to both target and source chains. The pallet itself has no assumptions about
relayers incentivization scheme, but it has some callbacks for paying rewards. See
[Integrating Message Lane Module into runtime](#Integrating-Message-Lane-Module-into-runtime)
for details.

Eventually, some relayer would notice this message in the "undelivered" state and it would decide to
deliver this message. Relayer then crafts `receive_messages_proof()` transaction (aka delivery
transaction) for the message lane module instance, deployed at the target chain. Relayer provides
his account id at the source chain, the proof of message (or several messages), the number of
messages in the transaction and their cumulative dispatch weight. Once a transaction is mined, the
message is considered "delivered".

Once a message is delivered, the relayer may want to confirm delivery back to the source chain.
There are two reasons why he would want to do that. The first is that we intentionally limit number
of "delivered", but not yet "confirmed" messages at inbound lanes
(see [What about other Constants in the Message Lane Module Configuration Trait](#What-about-other-Constants-in-the-Message-Lane-Module-Configuration-Trait) for explanation).
So at some point, the target chain may stop accepting new messages until relayers confirm some of
these. The second is that if the relayer wants to be rewarded for delivery, he must prove the fact
that he has actually delivered the message. And this proof may only be generated after the delivery
transaction is mined. So relayer crafts the `receive_messages_delivery_proof()` transaction (aka
confirmation transaction) for the message lane module instance, deployed at the source chain. Once
this transaction is mined, the message is considered "confirmed".

The "confirmed" state is the final state of the message. But there's one last thing related to the
message - the fact that it is now "confirmed" and reward has been paid to the relayer (or at least
callback for this has been called), must be confirmed to the target chain. Otherwise, we may reach
the limit of "unconfirmed" messages at the target chain and it will stop accepting new messages. So
relayer sometimes includes a nonce of the latest "confirmed" message in the next
`receive_messages_proof()` transaction, proving that some messages have been confirmed.

## Integrating Message Lane Module into Runtime

As it has been said above, the message lane module supports both outbound and inbound message lanes.
So if we will integrate a module in some runtime, it may act as the source chain runtime for
outbound messages and as the target chain runtime for inbound messages. In this section, we'll
sometimes refer to the chain we're currently integrating with, as this chain and the other chain as
bridged chain.

Message lane module doesn't simply accept transactions that are claiming that the bridged chain has
some updated data for us. Instead of this, the module assumes that the bridged chain is able to
prove that updated data in some way. The proof is abstracted from the module and may be of any kind.
In our Substrate-to-Substrate bridge we're using runtime storage proofs. Other bridges may use
transaction proofs, Substrate header digests or anything else that may be proved.

**IMPORTANT NOTE**: everything below in this chapter describes details of the message lane module
configuration. But if you interested in well-probed and relatively easy integration of two
Substrate-based chains, you may want to look at the
[bridge-runtime-common](../../bin/runtime-common/README.md) crate. This crate is providing a lot of
helpers for integration, which may be directly used from within your runtime. Then if you'll decide
to change something in this scheme, get back here for detailed information.

### General Information

The message lane module supports instances. Every module instance is supposed to bridge this chain
and some bridged chain. To bridge with another chain, using another instance is suggested (this
isn't forced anywhere in the code, though).

Message submitters may track message progress by inspecting module events. When Message is accepted,
the `MessageAccepted` event is emitted in the `send_message()` transaction. The event contains both
message lane identifier and nonce that has been assigned to the message. When a message is delivered
to the target chain, the `MessagesDelivered` event is emitted from the
`receive_messages_delivery_proof()` transaction. The `MessagesDelivered` contains the message lane
identifier and inclusive range of delivered message nonces.

### How to plug-in Message Lane Module to Send Messages to the Bridged Chain?

The `pallet_message_lane::Config` trait has 3 main associated types that are used to work with
outbound messages. The `pallet_message_lane::Config::TargetHeaderChain` defines how we see the
bridged chain as the target for our outbound messages. It must be able to check that the bridged
chain may accept our message - like that the message has size below maximal possible transaction
size of the chain and so on. And when the relayer sends us a confirmation transaction, this
implementation must be able to parse and verify the proof of messages delivery. Normally, you would
reuse the same (configurable) type on all chains that are sending messages to the same bridged
chain.

The `pallet_message_lane::Config::LaneMessageVerifier` defines a single callback to verify outbound
messages. The simplest callback may just accept all messages. But in this case you'll need to answer
many questions first. Who will pay for the delivery and confirmation transaction? Are we sure that
someone will ever deliver this message to the bridged chain? Are we sure that we don't bloat our
runtime storage by accepting this message? What if the message is improperly encoded or has some
fields set to invalid values? Answering all those (and similar) questions would lead to correct
implementation.

There's another thing to consider when implementing type for use in
`pallet_message_lane::Config::LaneMessageVerifier`. It is whether we treat all message lanes
identically, or they'll have different sets of verification rules? For example, you may reserve
lane#1 for messages coming from some 'wrapped-token' pallet - then you may verify in your
implementation that the origin is associated with this pallet. Lane#2 may be reserved for 'system'
messages and you may charge zero fee for such messages. You may have some rate limiting for messages
sent over the lane#3. Or you may just verify the same rules set for all outbound messages - it is
all up to the `pallet_message_lane::Config::LaneMessageVerifier` implementation.

The last type is the `pallet_message_lane::Config::MessageDeliveryAndDispatchPayment`. When all
checks are made and we have decided to accept the message, we're calling the
`pay_delivery_and_dispatch_fee()` callback, passing the corresponding argument of the `send_message`
function. Later, when message delivery is confirmed, we're calling `pay_relayers_rewards()`
callback, passing accounts of relayers and messages that they have delivered. The simplest
implementation of this trait is in the [`instant_payments.rs`](./src/instant_payments.rs) module and
simply calls `Currency::transfer()` when those callbacks are called. So `Currency` units are
transferred between submitter, 'relayers fund' and relayers accounts. Other implementations may use
more or less sophisticated techniques - the whole relayers incentivization scheme is not a part of
the message lane module.

### I have a Message Lane Module in my Runtime, but I Want to Reject all Outbound Messages. What shall I do?

You should be looking at the `bp_message_lane::source_chain::ForbidOutboundMessages` structure
[`bp_message_lane::source_chain`](../../primitives/message-lane/src/source_chain.rs). It implements
all required traits and will simply reject all transactions, related to outbound messages.

### How to plug-in Message Lane Module to Receive Messages from the Bridged Chain?

The `pallet_message_lane::Config` trait has 2 main associated types that are used to work with
inbound messages. The `pallet_message_lane::Config::SourceHeaderChain` defines how we see the
bridged chain as the source or our inbound messages. When relayer sends us a  delivery transaction,
this implementation must be able to parse and verify the proof of messages wrapped in this
transaction. Normally, you would reuse the same (configurable) type on all chains that are sending
messages to the same bridged chain.

The `pallet_message_lane::Config::MessageDispatch` defines a way on how to dispatch delivered
messages. Apart from actually dispatching the message, the implementation must return the correct
dispatch weight of the message before dispatch is called.

### I have a Message Lane Module in my Runtime, but I Want to Reject all Inbound Messages. What
shall I do?

You should be looking at the `bp_message_lane::target_chain::ForbidInboundMessages` structure from
the [`bp_message_lane::target_chain`](../../primitives/message-lane/src/target_chain.rs) module. It
implements all required traits and will simply reject all transactions, related to inbound messages.

### What about other Constants in the Message Lane Module Configuration Trait?

Message is being stored in the source chain storage until its delivery will be confirmed. After
that, we may safely remove the message from the storage. Lane messages are removed (pruned) when
someone sends a new message using the same lane. So the message submitter pays for that pruning. To
avoid pruning too many messages in a single transaction, there's
`pallet_message_lane::Config::MaxMessagesToPruneAtOnce` configuration parameter. We will never prune
more than this number of messages in the single transaction. That said, the value should not be too
big to avoid waste of resources when there are no messages to prune.

To be able to reward the relayer for delivering messages, we store a map of message nonces range =>
identifier of the relayer that has delivered this range at the target chain runtime storage. If a
relayer delivers multiple consequent ranges, they're merged into single entry. So there may be more
than one entry for the same relayer. Eventually, this whole map must be delivered back to the source
chain to confirm delivery and pay rewards. So to make sure we are able to craft this confirmation
transaction, we need to: (1) keep the size of this map below a certain limit and (2) make sure that
the weight of processing this map is below a certain limit. Both size and processing weight mostly
depend on the number of entries. The number of entries is limited with the
`pallet_message_lane::ConfigMaxUnrewardedRelayerEntriesAtInboundLane` parameter. Processing weight
also depends on the total number of messages that are being confirmed, because every confirmed
message needs to be read. So there's another
`pallet_message_lane::Config::MaxUnconfirmedMessagesAtInboundLane` parameter for that.

When choosing values for these parameters, you must also keep in mind that if proof in your scheme
is based on finality of headers (and it is the most obvious option for Substrate-based chains with
finality notion), then choosing too small values for these parameters may cause significant delays
in message delivery. That's because there too many actors involved in this scheme: 1) authorities
that are finalizing headers of the target chain need to finalize header with non-empty map; 2) the
headers relayer then needs to submit this header and its finality proof to the source chain; 3) the
messages relayer must then send confirmation transaction (storage proof of this map) to the source
chain; 4) when the confirmation transaction will be mined at some header, source chain authorities
must finalize this header; 5) the headers relay then needs to submit this header and its finality
proof to the target chain; 6) only now the messages relayer may submit new messages from the source
to target chain and prune the entry from the map.

Delivery transaction requires the relayer to provide both number of entries and total number of
messages in the map. This means that the module never charges an extra cost for delivering a map -
the relayer would need to pay exactly for the number of entries+messages it has delivered. So the
best guess for values of these parameters would be the pair that would occupy `N` percent of the
maximal transaction size and weight of the source chain. The `N` should be large enough to process
large maps, at the same time keeping reserve for future source chain upgrades.

## Non-Essential Functionality

Apart from the message related calls, the module exposes a set of auxiliary calls. They fall in two
groups, described in the next two paragraphs.

There may be a special account in every runtime where the message lane module is deployed. This
account, named 'module owner', is like a module-level sudo account - he's able to halt all and
result all module operations without requiring runtime upgrade. The module may have no message
owner, but we suggest to use it at least for initial deployment. To calls that are related to this
account are:
- `fn set_owner()`: current module owner may call it to transfer "ownership" to another account;
- `fn halt_operations()`: the module owner (or sudo account) may call this function to stop all
  module operations. After this call, all message-related transactions will be rejected until
  further `resume_operations` call'. This call may be used when something extraordinary happens with
  the bridge;
- `fn resume_operations()`: module owner may call this function to resume bridge operations. The
  module will resume its regular operations after this call.

Apart from halting and resuming the bridge, the module owner may also tune module configuration
parameters without runtime upgrades. The set of parameters needs to be designed in advance, though.
The module configuration trait has associated `Parameter` type, which may be e.g. enum and represent
a set of parameters that may be updated by the module owner. For example, if your bridge needs to
convert sums between different tokens, you may define a 'conversion rate' parameter and let the
module owner update this parameter when there are significant changes in the rate. The corresponding
module call is `fn update_pallet_parameter()`.

## Weights of Module Extrinsics

The main assumptions behind weight formulas is:
- all possible costs are paid in advance by the message submitter;
- whenever possible, relayer tries to minimize cost of its transactions. So e.g. even though sender
  always pays for delivering outbound lane state proof, relayer may not include it in the delivery
  transaction (unless message lane module on target chain requires that);
- weight formula should incentivize relayer to not to submit any redundant data in the extrinsics
  arguments;
- the extrinsic shall never be executing slower (i.e. has larger actual weight) than defined by the
  formula.

### Weight of `send_message` call

#### Related benchmarks

| Benchmark                         | Description                                         |
|-----------------------------------|-----------------------------------------------------|
`send_minimal_message_worst_case` | Sends 0-size message with worst possible conditions    |
`send_1_kb_message_worst_case`    | Sends 1KB-size message with worst possible conditions  |
`send_16_kb_message_worst_case`   | Sends 16KB-size message with worst possible conditions |

#### Weight formula

The weight formula is:
```
Weight = BaseWeight + MessageSizeInKilobytes * MessageKiloByteSendWeight
```

Where:

| Component                   | How it is computed?                                                          | Description                                                                                                                                  |
|-----------------------------|------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------|
| `SendMessageOverhead`       | `send_minimal_message_worst_case`                                            | Weight of sending minimal (0 bytes) message                    |
| `MessageKiloByteSendWeight` | `(send_16_kb_message_worst_case - send_1_kb_message_worst_case)/15` | Weight of sending every additional kilobyte of the message |

### Weight of `receive_messages_proof` call

#### Related benchmarks

| Benchmark                                               | Description*                                                                                                            |
|---------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| `receive_single_message_proof`                          | Receives proof of single `EXPECTED_DEFAULT_MESSAGE_LENGTH` message                                                      |
| `receive_two_messages_proof`                            | Receives proof of two identical `EXPECTED_DEFAULT_MESSAGE_LENGTH` messages                                              |
| `receive_single_message_proof_with_outbound_lane_state` | Receives proof of single `EXPECTED_DEFAULT_MESSAGE_LENGTH` message and proof of outbound lane state at the source chain |
| `receive_single_message_proof_1_kb`                     | Receives proof of single message. The proof has size of approximately 1KB**                                             |
| `receive_single_message_proof_16_kb`                    | Receives proof of single message. The proof has size of approximately 16KB**                                            |

*\* - In all benchmarks all received messages are dispatched and their dispatch cost is near to zero*

*\*\* - Trie leafs are assumed to have minimal values. The proof is derived from the minimal proof
by including more trie nodes. That's because according to `receive_message_proofs_with_large_leaf`
and `receive_message_proofs_with_extra_nodes` benchmarks, increasing proof by including more nodes
has slightly larger impact on performance than increasing values stored in leafs*.

#### Weight formula

The weight formula is:
```
Weight = BaseWeight + OutboundStateDeliveryWeight
       + MessagesCount * MessageDeliveryWeight
       + MessagesDispatchWeight
       + Max(0, ActualProofSize - ExpectedProofSize) * ProofByteDeliveryWeight
```

Where:

| Component                     | How it is computed?                                                                      | Description                                                                                                                                                                                                                                                                                                                                                                                         |
|-------------------------------|------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `BaseWeight`                  | `2*receive_single_message_proof - receive_two_messages_proof`                            | Weight of receiving and parsing minimal proof                                                                                                                                                                                                                                                                                                                                                       |
| `OutboundStateDeliveryWeight` | `receive_single_message_proof_with_outbound_lane_state - receive_single_message_proof`   | Additional weight when proof includes outbound lane state                                                                                                                                                                                                                                                                                                                                           |
| `MessageDeliveryWeight`       | `receive_two_messages_proof - receive_single_message_proof`                              | Weight of of parsing and dispatching (without actual dispatch cost) of every message                                                                                                                                                                                                                                                                                                                |
| `MessagesCount`               |                                                                                          | Provided by relayer                                                                                                                                                                                                                                                                                                                                                                                 |
| `MessagesDispatchWeight`      |                                                                                          | Provided by relayer                                                                                                                                                                                                                                                                                                                                                                                 |
| `ActualProofSize`             |                                                                                          | Provided by relayer                                                                                                                                                                                                                                                                                                                                                                                 |
| `ExpectedProofSize`           | `EXPECTED_DEFAULT_MESSAGE_LENGTH * MessagesCount + EXTRA_STORAGE_PROOF_SIZE`             | Size of proof that we are expecting. This only includes `EXTRA_STORAGE_PROOF_SIZE` once, because we assume that intermediate nodes likely to be included in the proof only once. This may be wrong, but since weight of processing proof with many nodes is almost equal to processing proof with large leafs, additional cost will be covered because we're charging for extra proof bytes anyway  |
| `ProofByteDeliveryWeight`     | `(receive_single_message_proof_16_kb - receive_single_message_proof_1_kb) / (15 * 1024)` | Weight of processing every additional proof byte over `ExpectedProofSize` limit                                                                                                                                                                                                                                                                                                                     |

#### Why for every message sent using `send_message` we will be able to craft `receive_messages_proof` transaction?

We have following checks in `send_message` transaction on the source chain:
- message size should be less than or equal to `2/3` of maximal extrinsic size on the target chain;
- message dispatch weight should be less than or equal to the `1/2` of maximal extrinsic dispatch
  weight on the target chain.

Delivery transaction is an encoded delivery call and signed extensions. So we have `1/3` of maximal
extrinsic size reserved for:
- storage proof, excluding the message itself. Currently, on our test chains, the overhead is always
  within `EXTRA_STORAGE_PROOF_SIZE` limits (1024 bytes);
- signed extras and other call arguments (`relayer_id: SourceChain::AccountId`, `messages_count:
  u32`, `dispatch_weight: u64`).

On Millau chain, maximal extrinsic size is `0.75 * 2MB`, so `1/3` is `512KB` (`524_288` bytes). This
should be enough to cover these extra arguments and signed extensions.

Let's exclude message dispatch cost from single message delivery transaction weight formula:
```
Weight = BaseWeight + OutboundStateDeliveryWeight + MessageDeliveryWeight
       + Max(0, ActualProofSize - ExpectedProofSize) * ProofByteDeliveryWeight
```

So we have `1/2` of maximal extrinsic weight to cover these components. `BaseWeight`,
`OutboundStateDeliveryWeight` and `MessageDeliveryWeight` are determined using benchmarks and are
hardcoded into runtime. Adequate relayer would only include required trie nodes into the proof. So
if message size would be maximal (`2/3` of `MaximalExtrinsicSize`), then the extra proof size would
be `MaximalExtrinsicSize / 3 * 2 - EXPECTED_DEFAULT_MESSAGE_LENGTH`.

Both conditions are verified by `pallet_message_lane::ensure_weights_are_correct` and
`pallet_message_lane::ensure_able_to_receive_messages` functions, which must be called from every
runtime's tests.

### Weight of `receive_messages_delivery_proof` call

#### Related benchmarks

| Benchmark                                                   | Description                                                                              |
|-------------------------------------------------------------|------------------------------------------------------------------------------------------|
| `receive_delivery_proof_for_single_message`                 | Receives proof of single message delivery                                                |
| `receive_delivery_proof_for_two_messages_by_single_relayer` | Receives proof of two messages delivery. Both messages are delivered by the same relayer |
| `receive_delivery_proof_for_two_messages_by_two_relayers`   | Receives proof of two messages delivery. Messages are delivered by different relayers    |

#### Weight formula

The weight formula is:
```
Weight = BaseWeight + MessagesCount * MessageConfirmationWeight
       + RelayersCount * RelayerRewardWeight
       + Max(0, ActualProofSize - ExpectedProofSize) * ProofByteDeliveryWeight
```

Where:

| Component                 | How it is computed?                                                                                                   | Description                                                                                                                                                                                             |
|---------------------------|-----------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `BaseWeight`              | `2*receive_delivery_proof_for_single_message - receive_delivery_proof_for_two_messages_by_single_relayer`             | Weight of receiving and parsing minimal delivery proof                                                                                                                                                  |
| `MessageDeliveryWeight`   | `receive_delivery_proof_for_two_messages_by_single_relayer - receive_delivery_proof_for_single_message`               | Weight of confirming every additional message                                                                                                                                                           |
| `MessagesCount`           |                                                                                                                       | Provided by relayer                                                                                                                                                                                     |
| `RelayerRewardWeight`     | `receive_delivery_proof_for_two_messages_by_two_relayers - receive_delivery_proof_for_two_messages_by_single_relayer` | Weight of rewarding every additional relayer                                                                                                                                                            |
| `RelayersCount`           |                                                                                                                       | Provided by relayer                                                                                                                                                                                     |
| `ActualProofSize`         |                                                                                                                       | Provided by relayer                                                                                                                                                                                     |
| `ExpectedProofSize`       | `EXTRA_STORAGE_PROOF_SIZE`                                                                                            | Size of proof that we are expecting                                                                                                                                                                     |
| `ProofByteDeliveryWeight` | `(receive_single_message_proof_16_kb - receive_single_message_proof_1_kb) / (15 * 1024)`                              | Weight of processing every additional proof byte over `ExpectedProofSize` limit. We're using the same formula, as for message delivery, because proof mechanism is assumed to be the same in both cases |

#### Why we're always able to craft `receive_messages_delivery_proof` transaction?

There can be at most `<PeerRuntime as pallet_message_lane::Config>::MaxUnconfirmedMessagesAtInboundLane`
messages and at most
`<PeerRuntime as pallet_message_lane::Config>::MaxUnrewardedRelayerEntriesAtInboundLane` unrewarded
relayers in the single delivery confirmation transaction.

We're checking that this transaction may be crafted in the
`pallet_message_lane::ensure_able_to_receive_confirmation` function, which must be called from every
runtime' tests.
