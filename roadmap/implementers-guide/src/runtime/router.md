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
/// The downward messages addressed for a certain para.
DownwardMessageQueues: map ParaId => Vec<DownwardMessage>; // TODO: augment this with a block number
DmqWatermarks: map ParaId => Option<BlockNumber>'
```

### HRMP

HRMP related structs:

```rust,ignore
struct HrmpOpenChannelRequest {
    sender: ParaId,
    recipient: ParaId,
    confirmed: bool,
    age: u32,
}

struct HrmpCloseChannelRequest {
    // invariant: equals either to sender or recipient.
    initiator: ParaId,
    sender: ParaId,
    recipient: ParaId,
}

struct HrmpChannel {
    // deposits taken from both sides.
    // consider merging if symmetrical.
    sender_deposit: Balance,
    recipient_deposit: Balance,

    // The number of messages placed in the channel by the sender and the total number of bytes
    // the occupy.
    used_places: u32,
    used_bytes: u32,

    // if a channel is sealed, then it doesn't accept new messages.
    // If it is sealed and `used_places` reaches 0 then the channel
    // can be removed at the next session boundary.
    sealed: bool,
}
```
HRMP related storage layout

```rust,ignore
// TODO: Proposal: place the primary information into a map `(sender, recipient)`.
// then add indexes?

// TODO: Number of open requests per sender.
// TODO: We need to iterate over all open requests.
// TODO: We need to iterate over all close requests.
// TODO: There should be a set for quickly checking if a close request for `(sender, reciever)` exists.
hrmp_open_ch_requests: Vec<HrmpOpenChannelRequest>;
hrmp_close_ch_requests: Vec<HrmpCloseChannelRequest>;

// TODO: Number of opened channels per sender.
// TODO: We need to be able to remove all channels for a particular sender
// TODO: We need to be able to remove all channels for a particular recipient
channels: map (ParaId, ParaId) => Option<Channel>;
```

## Initialization

No initialization routine runs for this module.

## Routines

The following routines are intended to be invoked by paras' upward messages.

* `init_open_channel(recipient)`:
  1. Check that the origin of this message is a para. We say that the origin of this message is the `sender`.
  1. Check that the `sender` is not `recipient`.
  1. Check that the `recipient` para exists.
  1. Check that there is no existing intention to open a channel between sender and recipient. TODO: Be more specific
  1. Check that the sum of the number of already opened HRMP channels by the `sender` (TODO: be more specific) and the number of open requests by the `sender` (TODO: be more specific) doesn't exceed the limit of channels minus 1.
  1. Reserve the deposit for the `sender`.
  1. Add a new entry to `hrmp_open_ch_requests` and all other auxilary structures (TODO: be more specific)

* `accept_open_channel(i)`, `i` - is the index of open channel request: (TODO: consider specifying the pair directly)
  1. Check that the designated open channel request exists
  1. Check that the request's `recipient` corresponds to the origin of this message.
  1. Reserve the deposit for the `recipient`.
  1. Set the request's `confirmed` flag to `true`.

* `close_channel(sender, recipient)`:
  1. Check that the channel between `sender` and `recipient` exists
  1. Check that the channel between `sender` and `recipient` is not already sealed
  1. Check that the origin of the message is either `sender` or `recipient`
  1. Check that there is no existing intention to close the channel between `sender` and `recipient`. (TODO: be more specific)
  1. Add a new entry to `hrmp_close_ch_requests`.

The routines described below are for use within a session change and called by the `Paras` module.

* `offboard_para(P: ParaId)`:
  1. Remove all inbound channels of `P`, i.e. `(_, P)`,
  1. Remove all outbound channels of `P`, i.e. `(P, _)`,
  - Note that we don't remove the open/close requests since they are gon die out naturally.
TODO: What happens with the deposit?

Candidate Acceptance Function:

* `check_dmq_watermark(P: ParaId, new_dmq_watermark)`:
  1. `new_dmq_watermark` should be strictly greater than the value of `DmqWatermarks` for `P` (if any).
  1. `new_dmq_watermark` must not be greater than the context's block number.
  1. TODO: new_dmq_watermark should point either to the context's block number or to a block number such that a message exist with that block number in DMQ.
* `verify_outbound_hrmp(sender: ParaId, Vec<HorizontalMessage>)`:
  1. For each horizontal message `M` with the channel `C` identified by `(sender, M.recipient)` check:
    1. exists
	1. `C.sealed = false`
	1. `M`'s payload size summed with the `C.used_bytes` doesn't exceed a preconfigured limit (TODO: be more specific about the limit)
	1. `C.used_places + 1` doesn't exceed a preconfigured limit (TODO: be more specific about the limit)

Enactment: TODO:

* `queue_horizontal_messages(sender: ParaId, Vec<HorizontalMessage>)`:
  1. For each horizontal message `HM` with the channel `C` identified by `(sender, HM.recipient)`:
	1. Using the payload from `HM` and the `sender` creates a downward message `DM` of the kind `HorizontalMessage`.
    1. Appends `DM` into the `DownwardMessageQueues` corresponding to `HM.recipient`.
    1. Increment `C.used_places`
	1. Increment `C.used_bytes` by `HM`'s payload size
* `prune_dmq(recipient, new_dmq_watermark)`:
  1. Prune `DownwardMessageQueues` for `recipient` up to `new_dmq_watermark`. For each pruned DMQ message of kind `HorizontalMessage` `M` with the channel `C` identified by `(M.sender, recipient)`.
	1. Decrement `C.used_places`
	1. Decrement `C.used_bytes` by `M`'s payload size.
  1. Set `DmqWatermarks` for `P` to be equal to `new_dmq_watermark`


* `queue_upward_messages(ParaId, Vec<UpwardMessage>)`:
  1. Updates `NeedsDispatch`, and enqueues upward messages into `RelayDispatchQueue` and modifies the respective entry in `RelayDispatchQueueSize`.

## Session Change

1. For each request `R` in the open channel request list: (TODO: Be more specific)
    1. if `R.confirmed = false`:
        1. increment `R.age` by 1.
        2. if `R.age` reached a preconfigured time-to-live limit, then (TODO: what preconfigured limit? global or local to para?)
            1. refund `R.sender_deposit` to the sender
            2. remove `R`
    2. if `R.confirmed = true`,
        1. create a new channel between sender → recipient (TODO: Be more specific) (TODO: ensure that neither of the participants were offboarded?)
        2. remove `R`
1. For each request `R` in the close channel request list. A channel `C` identified by `(R.sender, R.recipient)`:
    1. If `C.used_places` is greater than 0 then set `C.sealed = true`
    1. Otherwise, remove the channel eagerly (See below)
1. For each channel `C` in `channels` remove if `C.sealed = true` and `C.used_places = 0`. (TODO: how to iterate? Make this efficient)

To remove a channel `C` identified with a tuple `(sender, recipient)`:

1. Return `C.sender_deposit` to the `sender`.
1. Return `C.recipient_deposit` to the `recipient`.
1. Remove `C` from `channels`.


## Finalization

  1. Dispatch queued upward messages from `RelayDispatchQueues` in a FIFO order applying the `config.watermark_upward_queue_size` and `config.max_upward_queue_count` limits.
