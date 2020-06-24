# Router Module

The Router module is responsible for storing and dispatching Upward and Downward messages from and to parachains respectively. It is intended to later handle the XCMP logic as well.

For each enacted block the `queue_upward_messages` entry-point is called.

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
```

## Initialization

No initialization routine runs for this module.

## Routines

* `queue_upward_messages(ParaId, Vec<UpwardMessage>)`:
  1. Updates `NeedsDispatch`, and enqueues upward messages into `RelayDispatchQueue` and modifies the respective entry in `RelayDispatchQueueSize`.

## Finalization

  1. Dispatch queued upward messages from `RelayDispatchQueues` in a FIFO order applying the `config.watermark_upward_queue_size` and `config.max_upward_queue_count` limits.
