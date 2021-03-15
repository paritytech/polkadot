# High-Level Bridge Documentation

## Purpose

Trustless connecting between two Substrate-based chains using GRANDPA finality.

## Overview

Even though we support two-way bridging, the documentation will generally talk about a one-sided
interaction. That's to say, we will only talk about syncing headers and messages from a _source_
chain to a _target_ chain. This is because the two-sided interaction is really just the one-sided
interaction with the source and target chains switched.

To understand the full interaction with the bridge, take a look at the
[testing scenarios](./testing-scenarios.md) document. It describes potential use cases and describes
how each of the layers outlined below is involved.

The bridge is built from various components. Here is a quick overview of the important ones.

### Header Sync

A light client of the source chain built into the target chain's runtime. It is a single FRAME
pallet. It provides a "source of truth" about the source chain headers which have been finalized.
This is useful for higher level applications.

### Headers Relayer

A standalone application connected to both chains. It submits every source chain header it sees to
the target chain through RPC.

### Message Delivery

A FRAME pallet built on top of the header sync pallet. It allows users to submit messages to the
source chain, which are to be delivered to the target chain. The delivery protocol doesn't care
about the payload more than it has to. Handles replay protection and message ordering.

### Message Dispatch

A FRAME pallet responsible for interpreting the payload of delivered messages.

### Message Relayer

A standalone application handling delivery of the messages from source chain to the target chain.

## Processes

High level sequence charts of the process can be found in [a separate document](./high-level.html).

### Substrate (GRANDPA) Header Sync

The header sync pallet (`pallet-substrate-bridge`) is an on-chain light client for chains which use
GRANDPA finality. It is part of the target chain's runtime, and accepts headers from the source
chain. Its main goals are to accept valid headers, track GRANDPA finality set changes, and verify
GRANDPA finality proofs (a.k.a justifications).

The pallet does not care about what block production mechanism is used for the source chain
(e.g Aura or BABE) as long as it uses the GRANDPA finality gadget. Due to this it is possible for
the pallet to import (but not necessarily finalize) headers which are _not_ valid according to the
source chain's block production mechanism.

The pallet has support for tracking forks and uses the longest chain rule to determine what the
canonical chain is. The pallet allows headers to be imported on a different fork from the canonical
one as long as the headers being imported don't conflict with already finalized headers (for
example, it will not allow importing a header at a lower height than the best finalized header).

When tracking authority set changes, the pallet - unlike the full GRANDPA protocol - does not
support tracking multiple authority set changes across forks. Each fork can have at most one pending
authority set change. This is done to prevent DoS attacks if GRANDPA on the source chain were to
stall for a long time (the pallet would have to do a lot of expensive ancestry checks to catch up).

Referer to the [pallet documentation](../modules/substrate/src/lib.rs) for more details.

#### Header Relayer strategy

There is currently no reward strategy for the relayers at all. They also are not required to be
staked or registered on-chain, unlike in other bridge designs. We consider the header sync to be
an essential part of the bridge and the incentivisation should be happening on the higher layers.

At the moment, signed transactions are the only way to submit headers to the header sync pallet.
However, in the future we would like to use  unsigned transactions for headers delivery. This will
allow transaction de-duplication to be done at the transaction pool level and also remove the cost
for message relayers to run header relayers.

### Message Passing

Once header sync is maintained, the target side of the bridge can receive and verify proofs about
events happening on the source chain, or its internal state. On top of this, we built a message
passing protocol which consists of two parts described in following sections: message delivery and
message dispatch.

#### Message Lanes Delivery

The [Message delivery pallet](../modules/message-lane/src/lib.rs) is responsible for queueing up
messages and delivering them in order on the target chain. It also dispatches messages, but we will
cover that in the next section.

The pallet supports multiple lanes (channels) where messages can be added. Every lane can be
considered completely independent from others, which allows them to make progress in parallel.
Different lanes can be configured to validated messages differently (e.g higher rewards, specific
types of payload, etc.) and may be associated with a particular "user application" built on top of
the bridge. Note that messages in the same lane MUST be delivered _in the same order_ they were
queued up.

The message delivery protocol does not care about the payload it transports and can be coupled
with an arbitrary message dispatch mechanism that will interpret and execute the payload if delivery
conditions are met. Each delivery on the target chain is confirmed back to the source chain by the
relayer. This is so that she can collect the reward for delivering these messages.

Users of the pallet add their messages to an "outbound lane" on the source chain. When a block is
finalized message relayers are responsible for reading the current queue of messages and submitting
some (or all) of them to the "inbound lane" of the target chain. Each message has a `nonce`
associated with it, which serves as the ordering of messages. The inbound lane stores the last
delivered nonce to prevent replaying messages. To succesfuly deliver the message to the inbound lane
on target chain the relayer has to present present a storage proof which shows that the message was
part of the outbound lane on the source chain.

During delivery of messages they are immediately dispatched on the target chain and the relayer is
required to declare the correct `weight` to cater for all messages dispatch and pay all required
fees of the target chain. To make sure the relayer is incentivised to do so, on the source chain:
- the user provides a declared dispatch weight of the payload
- the pallet calculates the expected fee on the target chain based on the declared weight
- the pallet converts the target fee into source tokens (based on a price oracle) and reserves
  enough tokens to cover for the delivery, dispatch, confirmation and additional relayers reward.

If the declared weight turns out to be too low on the target chain the message is delivered but
it immediately fails to dispatch. The fee and reward is collected by the relayer upon confirmation
of delivery.

Due to the fact that message lanes require delivery confirmation transactions, they also strictly
require bi-directional header sync (i.e. you can't use message delivery with one-way header sync).

#### Dispatching Messages

The [Message dispatch pallet](../modules/call-dispatch/src/lib.rs) is used to perform the actions
specified by messages which have come over the bridge. For Substrate-based chains this means
interpreting the source chain's message as a `Call` on the target chain.

An example `Call` of the target chain would look something like this:

```rust
target_runtime::Call::Balances(target_runtime::pallet_balances::Call::transfer(recipient, amount))
```

When sending a `Call` it must first be SCALE encoded and then sent to the source chain. The `Call`
is then delivered by the message lane delivery mechanism from the source chain to the target chain.
When a message is received the inbound message lane on the target chain will try and decode the
message payload into a `Call` enum. If it's successful it will be dispatched after we check that the
weight of the call does not exceed the weight declared by the sender. The relayer pays fees for
executing the transaction on the target chain, but her costs should be covered by the sender on the
source chain.

When dispatching messages there are three Origins which can be used by the target chain:
1. Root Origin
2. Source Origin
3. Target Origin

Senders of a message can indicate which one of the three origins they would like to dispatch their
message with. However, there are restrictions on who/what is allowed to dispatch messages with a
particular origin.

The Root origin represents the source chain's Root account on the target chain. This origin can can
only be dispatched on the target chain if the "send message" request was made by the Root origin of
the source chain - otherwise the message will fail to be dispatched.

The Source origin represents an account without a private key on the target chain. This account will
be generated/derived using the account ID of the sender on the source chain. We don't necessarily
require the source account id to be associated with a private key on the source chain either. This
is useful for representing things such as source chain proxies or pallets.

The Target origin represents an account with a private key on the target chain. The sender on the
source chain needs to prove ownership of this account by using their target chain private key to
sign: `(Call, SourceChainAccountId).encode()`. This will be included in the message payload and
verified by the target chain before dispatch.

See [`CallOrigin` documentation](../modules/call-dispatch/src/lib.rs) for more details.

#### Message Relayers Strategy
