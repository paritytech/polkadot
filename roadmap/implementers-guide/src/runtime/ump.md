# UMP Module

A module responsible for Upward Message Passing (UMP). See [Messaging Overview](../messaging.md) for more details.

## Storage

Storage related to UMP

```rust
/// The messages waiting to be handled by the relay-chain originating from a certain parachain.
///
/// Note that some upward messages might have been already processed by the inclusion logic. E.g.
/// channel management messages.
///
/// The messages are processed in FIFO order.
RelayDispatchQueues: map ParaId => Vec<UpwardMessage>;
/// Size of the dispatch queues. Caches sizes of the queues in `RelayDispatchQueue`.
///
/// First item in the tuple is the count of messages and second
/// is the total length (in bytes) of the message payloads.
///
/// Note that this is an auxilary mapping: it's possible to tell the byte size and the number of
/// messages only looking at `RelayDispatchQueues`. This mapping is separate to avoid the cost of
/// loading the whole message queue if only the total size and count are required.
///
/// Invariant:
/// - The set of keys should exactly match the set of keys of `RelayDispatchQueues`.
RelayDispatchQueueSize: map ParaId => (u32, u32); // (num_messages, total_bytes)
/// The ordered list of `ParaId`s that have a `RelayDispatchQueue` entry.
///
/// Invariant:
/// - The set of items from this vector should be exactly the set of the keys in
///   `RelayDispatchQueues` and `RelayDispatchQueueSize`.
NeedsDispatch: Vec<ParaId>;
/// This is the para that gets dispatched first during the next upward dispatchable queue
/// execution round.
///
/// Invariant:
/// - If `Some(para)`, then `para` must be present in `NeedsDispatch`.
NextDispatchRoundStartWith: Option<ParaId>;
```


## Initialization

No initialization routine runs for this module.

## Routines

Candidate Acceptance Function:

* `check_upward_messages(P: ParaId, Vec<UpwardMessage>`):
    1. Checks that there are at most `config.max_upward_message_num_per_candidate` messages.
    1. Checks that no message exceeds `config.max_upward_message_size`.
    1. Verify that queuing up the messages could not result in exceeding the queue's footprint
    according to the config items. The queue's current footprint is provided in well_known_keys
    in order to facilitate oraclisation on to the para.

Candidate Enactment:

* `receive_upward_messages(P: ParaId, Vec<UpwardMessage>)`:
    1. Process each upward message `M` in order:
        1. Place in the dispatch queue according to its para ID (or handle it immediately).

## Session Change

1. Nothing specific needs to be done, however the channel's dispatch queue may possibly be "swept"
which would prevent the dispatch queue from automatically being serviced. This is a consideration
for the chain and specific behaviour is not defined.
