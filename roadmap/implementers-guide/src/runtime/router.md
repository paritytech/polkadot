# Router Module

The Router module is responsible for all messaging mechanisms supported between paras and the relay chain, specifically: UMP, DMP, HRMP and later XCMP.

## Storage

Storage layout:

```rust,ignore
/// Messages ready to be dispatched onto the relay chain.
/// This is subject to `max_upward_queue_count` and
/// `watermark_queue_size` from `HostConfiguration`.
RelayDispatchQueues: map ParaId => Vec<UpwardMessage>;
/// Size of the dispatch queues. Caches sizes of the queues in `RelayDispatchQueue`.
/// First item in the tuple is the count of messages and second
/// is the total length (in bytes) of the message payloads.
RelayDispatchQueueSize: map ParaId => (u32, u32);
/// The ordered list of `ParaId`s that have a `RelayDispatchQueue` entry.
NeedsDispatch: Vec<ParaId>;
/// The mapping that tracks how many bytes / messages are sent by a certain sender - recipient pair.
///
/// First item in the tuple is the count of messages for the (sender, recipient) pair and the second
/// item is the total length (in bytes) of the message payloads.
HorizontalMessagesResourceUsage: map (ParaId, ParaId) => (u32, u32);
/// The downward messages addressed for a certain para. These vectors are not bounded directly, but
/// rather each possible sender can put only a limited amount of messages in the downward queue.
DownwardMessageQueues: map ParaId => Vec<DownwardMessage>;
/// The number of downward messages originated from the relay chain to a certain para. This is subject
/// to the `max_relay_chain_downward_messages` limit found in `HostConfiguration`.
RelayChainDownwardMessages: map ParaId => u32;
```

## Initialization

No initialization routine runs for this module.

## Routines

There are situations when actions that took place within the relay chain could lead to a downward message
sent to a para. For example, if an entry-point to transfer some funds to a para was called.

For these cases, there are two routines, `has_dmq_capacity_for_relay_chain` and `send_downward_messages`,
intended for use by the relay chain.

`send_downward_messages` is used for enqueuing one or more downward messages for a certain recipient. Since downward
message queues can hold only so many messages per one sender (and the relay chain is not an exception),
`send_downward_messages` can fail refusing enqueuing a message that would have exceeded the limit. In those cases
`has_dmq_capacity_for_relay_chain` can be used for checking in advance if there is enough space for a given
number of messages.

Note that an HRMP message can only be sent by para candidates.

* `has_dmq_capacity_for_relay_chain(recipient: ParaId, n: u32)`.
  1. Checks that the sum of the number `RelayChainDownwardMessages` for `recipient` and `n` is less
  than or equal to `config.max_relay_chain_downward_messages`.
* `send_downward_messages(recipient: ParaId, Vec<DownwardMessage>)`.
  1. Checks that there is enough capacity in the receipient's downward queue using `has_dmq_capacity_for_relay_chain`.
  1. For each downward message `DM`:
    1. Checks that `DM` is not of type `HorizontalMessage`.
    1. Appends `DM` into the `DownwardMessageQueues` corresponding to `recipient`.
  1. Increments `RelayChainDownwardMessages` for the `recipient` according to the number of messages sent.

The following routines are intended for use during the course of inclusion or enactment of para candidates.
For checking the validity of message passing within a candidate the `ensure_processed_downward_messages`
and `ensure_horizontal_messages_fit` routines are called. When a candidate is enacted the
`drain_downward_messages`, `queue_horizontal_messages` and `queue_upward_messages` are called.

* `ensure_processed_downward_messages(recipient: ParaId, processed_downward_messages: u32)`:
  1. Checks that `DownwardMessageQueues` for `recipient` is at least `processed_downward_messages` long.
  1. Checks that `processed_downward_messages` is at least 1 if `DownwardMessageQueues` for `recipient` is not empty.
* `ensure_horizontal_messages_fit(sender, Vec<HorizontalMessage>)`:
  1. For each horizontal message `HM`, with recipient `R`:
    1. Fetches the current usage level for the pair `(sender, R)`. The usage level is defined by
    a tuple of `(msg_count, total_byte_size)`.
    1. Checks that `msg_count + 1` is less or equal than `config.max_hrmp_queue_count_per_sender`.
    1. Checks that the sum of the payload size occupied by `HM` and `total_byte_size` is less than or
    equal to `config.max_hrmp_queue_size_per_sender`.
* `drain_downward_messages(recipient: ParaId, processed_downward_messages)`:
  1. Prunes `processed_downward_messages` from the beginning of the downward message queue. For each pruned message `DM`:
    1. If `DM` is a horizontal message sent from a sender `S`,
      1. With the mapping from `HorizontalMessagesResourceUsage` that corresponds to `(S, recipient)` represented by
      `(msg_count, total_byte_size)`.
          1. Decrements `msg_count` by 1.
          1. Decrements `total_byte_size` according to the payload size of `DM`.
    1. Otherwise, decrements `RelayChainDownwardMessages` for the `recipient`.
* `queue_horizontal_messages(sender: ParaId, Vec<HorizontalMessage>)`:
  1. For each horizontal message `HM`, with recipient `R`:
    1. Using the payload from `HM` and the `sender` creates a downward message `DM`.
    1. Appends `DM` into the `DownwardMessageQueues` corresponding to `R`.
    1. With the mapping from `HorizontalMessagesResourceUsage` that corresponds to `(sender, R)` represented by
      `(msg_count, total_byte_size)`.
        1. Increment `msg_count` by 1.
        1. Increment `total_byte_size` according to the payload size of `DM`.
* `queue_upward_messages(ParaId, Vec<UpwardMessage>)`:
  1. Updates `NeedsDispatch`, and enqueues upward messages into `RelayDispatchQueue` and modifies the respective entry in `RelayDispatchQueueSize`.

## Finalization

  1. Dispatch queued upward messages from `RelayDispatchQueues` in a FIFO order applying the `config.watermark_upward_queue_size` and `config.max_upward_queue_count` limits.
