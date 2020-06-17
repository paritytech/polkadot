# Router Module

The Router module is responsible for storing and dispatching Upwards and Downwards messages from and to parachains respectively. It is intended to later handle the XCMP logic as well.

For each enacted block the `enqueue_upward_messages` entry-point is called. All dispatching logic is done by the `dispatch_upward_messages` entry-point. This entry-point is mandatory, in that it must be envoked once within every block.

This module does not have the same initialization/finalization concerns as the others, it requires that its entry points be triggered
  1. When candidate is enacted.
  1. Upon new block initialization.

## Storage

Storage layout:

```rust
/// Messages ready to be dispatched onto the relay chain.
/// This is subject to `max_upwards_queue_count` and
///`watermark_queue_size` from `HostConfiguration`.
RelayDispatchQueues: map ParaId => Vec<UpwardMessage>;
/// Size of the dispatch queues. Caches sizes of the queues in `RelayDispatchQueue`.
/// First item in the tuple is the count of messages and second
/// is the total length (in bytes) of the message payloads.
RelayDispatchQueueSize: map ParaId => (u32, u32);
/// The ordered list of `ParaId`s that have a `RelayDispatchQueue` entry.
NeedsDispatch: Vec<ParaId>;
```

## Routines

* `queue_upward_messages(AbridgedCandidateReceipt)`:
  1. Updates `NeedsDispatch`, and enqueues upward messages into `RelayDispatchQueue` and modifies the respective entry in `RelayDispatchQueueSize`.
* `dispatch_upward_messages()`:
  1. If `NeedsDispatch` contains an entry passed as an input parameter start dispatching messages from it's respective entry in `RelayDispatchQueues`. The dispatch is done in the FIFO order and it drains the queue and removes it from `RelayDispatchQueues`.

## Initialization

  > TODO: On initalization or finalization or when exactly?
  1. Dispatch queued upward messages using `dispatch_upward_messages`.
