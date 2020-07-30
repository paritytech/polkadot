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
DownwardMessageQueues: map ParaId => Vec<DownwardMessage>;
```

### HRMP

HRMP related structs:

```rust,ignore
/// A type used to designate a HRMP channel between a (sender, recipient).
type HrmpChannelId = (ParaId, ParaId);

struct HrmpOpenChannelRequest {
    sender: ParaId,
    recipient: ParaId,
    confirmed: bool,
    age: u32,
	sender_deposit: Balance,
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
HrmpOpenChannelRequests: Vec<HrmpOpenChannelRequest>;
HrmpCloseChannelRequests: Vec<HrmpCloseChannelRequest>;

HrmpWatermarks: map ParaId => Option<BlockNumber>'

// TODO: Number of opened channels per sender.
// TODO: We need to be able to remove all channels for a particular sender
// TODO: We need to be able to remove all channels for a particular recipient
HrmpChannels: map HrmpChannelId => Option<Channel>;

HrmpChannelContents: map HrmpChannelId => Vec<(BlockNumber, Vec<u8>)>;
/// Maintains a mapping that can be used to answer a question:
/// What paras sent a message at the given block number for a given reciever.
HrmpChannelDigests: map ParaId => Vec<(BlockNumber, Vec<ParaId>)>;

/// A list of channels that have `sealed = true` and `used_places = 0`. Those will be removed at
/// the nearest session boundary.
HrmpCondemnedChannels: Vec<HrmpChannelId>;
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
  1. Add a new entry to `HrmpOpenChannelRequests` and all other auxilary structures (TODO: be more specific)

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
  1. Add a new entry to `HrmpCloseChannelRequests`.

Candidate Acceptance Function:

* `check_processed_downward_messages(P: ParaId, processed_downward_messages)`:
  1. Checks that `DownwardMessageQueues` for `P` is at least `processed_downward_messages` long.
  1. Checks that `processed_downward_messages` is at least 1 if `DownwardMessageQueues` for `recipient` is not empty.
* `check_hrmp_watermark(P: ParaId, new_hrmp_watermark)`:
  1. `new_hrmp_watermark` should be strictly greater than the value of `HrmpWatermarks` for `P` (if any).
  1. `new_hrmp_watermark` must not be greater than the context's block number.
  1. in ``HrmpChannelDigests`` for `P` an entry with the block number equal to `new_hrmp_watermark` should exist.
* `verify_outbound_hrmp(sender: ParaId, Vec<HorizontalMessage>)`:
  1. For each horizontal message `M` with the channel `C` identified by `(sender, M.recipient)` check:
    1. exists
	1. `C.sealed = false`
	1. `M`'s payload size summed with the `C.used_bytes` doesn't exceed a preconfigured limit (TODO: be more specific about the limit)
	1. `C.used_places + 1` doesn't exceed a preconfigured limit (TODO: be more specific about the limit)

Candidate Enactment:

* `queue_horizontal_messages(sender: ParaId, Vec<HorizontalMessage>)`:
  1. For each horizontal message `HM` with the channel `C` identified by `(sender, HM.recipient)`:
	1. Append `HM` into `HrmpChannelContents` that corresponds to `C`.
	1. Locate or create an entry in ``HrmpChannelDigests`` for `HM.recipient` and append `sender` into the entry's list.
    1. Increment `C.used_places`
	1. Increment `C.used_bytes` by `HM`'s payload size
* `prune_hrmp(recipient, new_hrmp_watermark)`:
  1. From ``HrmpChannelDigests`` for `recipient` remove all entries up to an entry with block number equal to `new_hrmp_watermark`.
  1. From the removed digests construct a set of paras that sent new messages within the interval between the old and new watermarks.
  1. For each channel `C` identified by `(sender, recipient)` for each `sender` coming from the set, prune messages up to the `new_hrmp_watermark`.
  1. For each pruned message `M` from channel `C`:
	1. Decrement `C.used_places`
	1. Decrement `C.used_bytes` by `M`'s payload size.
  1. If `C.used_places = 0` and `C.sealed = true`, append `C` to `HrmpCondemnedChannels`.
  1. Set `HrmpWatermarks` for `P` to be equal to `new_hrmp_watermark`
* `prune_dmq(P: ParaId, processed_downward_messages)`:
  1. Remove the first `processed_downward_messages` from the `DownwardMessageQueues` of `P`.

* `queue_upward_messages(ParaId, Vec<UpwardMessage>)`:
  1. Updates `NeedsDispatch`, and enqueues upward messages into `RelayDispatchQueue` and modifies the respective entry in `RelayDispatchQueueSize`.

The routines described below are for use within a session change and called by the `Paras` module.

* `offboard_para(P: ParaId)`:
  1. Remove all inbound channels of `P`, i.e. `(_, P)`,
  1. Remove all outbound channels of `P`, i.e. `(P, _)`,
  - Note that we don't remove the open/close requests since they are gon die out naturally.
TODO: What happens with the deposit?

## Session Change

1. For each request `R` in the open channel request list: (TODO: Be more specific)
    1. if `R.confirmed = false`:
        1. increment `R.age` by 1.
        1. if `R.age` reached a preconfigured time-to-live limit, then (TODO: what preconfigured limit? global or local to para?)
            1. refund `R.sender_deposit` to the sender
            1. remove `R`
    2. if `R.confirmed = true`,
		1. check that `R.sender` and `R.recipient` are not offboarded.
        1. create a new channel `C` between `(R.sender, R.recipient)`. Initialize the `C.sender_deposit` with `R.sender_deposit` and `C.recipient_deposit` with the value found in the configuration. (TODO: be more specific about the configuration)
        1. remove `R`
1. For each request `R` in the close channel request list. A channel `C` identified by `(R.sender, R.recipient)`:
    1. If `C.used_places` is greater than 0 then set `C.sealed = true`
    1. Otherwise, remove the channel eagerly (See below)
1. Remove all channels found in the `HrmpCondemnedChannels` list and clear the list.

To remove a channel `C` identified with a tuple `(sender, recipient)`:

1. Return `C.sender_deposit` to the `sender`.
1. Return `C.recipient_deposit` to the `recipient`.
1. Remove `C` from `HrmpChannels`.


## Finalization

  1. Dispatch queued upward messages from `RelayDispatchQueues` in a FIFO order applying the `config.watermark_upward_queue_size` and `config.max_upward_queue_count` limits.
