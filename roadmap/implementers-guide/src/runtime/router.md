# Router Module

The Router module is responsible for all messaging mechanisms supported between paras and the relay chain, specifically: UMP, DMP, HRMP and later XCMP.

## Storage

Storage layout:

```rust,ignore
/// Paras that are to be cleaned up at the end of the session.
/// The entries are sorted ascending by the para id.
OutgoingParas: Vec<ParaId>;
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
struct HrmpChannelId {
    sender: ParaId,
    recipient: ParaId,
}

/// A description of a request to open an HRMP channel.
struct HrmpOpenChannelRequest {
    /// The sender and the initiator of this request.
    sender: ParaId,
    /// The recipient of the opened request.
    recipient: ParaId,
    /// Indicates if this request was confirmed by the recipient.
    confirmed: bool,
    /// How many session boundaries ago this request was seen.
    age: SessionIndex,
    /// The amount that the sender supplied at the time of creation of this request.
    sender_deposit: Balance,
    /// The maximum number of messages that can be pending in the channel at once.
    limit_used_places: u32,
    /// The maximum total size of the messages that can be pending in the channel at once.
    limit_used_bytes: u32,
}

/// A description of a request to close an opened HRMP channel.
struct HrmpCloseChannelRequest {
    /// The para which initiated closing an existing channel.
    ///
    /// Invariant: it should be equal to one of the following
    /// - `Some(sender)`
    /// - `Some(recipient)`
    /// - `None` in case the request initiated by the runtime (including governance, migration, etc)
    initiator: Option<ParaId>,
    /// The identifier of an HRMP channel to be closed.
    id: HrmpChannelId,
}

/// A metadata of an HRMP channel.
struct HrmpChannel {
    /// The amount that the sender supplied as a deposit when opening this channel.
    sender_deposit: Balance,
    /// The amount that the recipient supplied as a deposit when accepting opening this channel.
    recipient_deposit: Balance,
    /// The maximum number of messages that can be pending in the channel at once.
    limit_used_places: u32,
    /// The maximum total size of the messages that can be pending in the channel at once.
    limit_used_bytes: u32,
    /// The current number of messages pending in the channel.
    /// Invariant: should be less or equal to `limit_used_places`.
    used_places: u32,
    /// The total size in bytes of all message payloads in the channel.
    /// Invariant: should be less or equal to `limit_used_bytes`.
    used_bytes: u32,
    /// A head of the Message Queue Chain for this channel. Each link in this chain has a form:
    /// `(prev_head, B, H(M))`, where
    /// - `prev_head`: is the previous value of `mqc_head`.
    /// - `B`: is the [relay-chain] block number in which a message was appended
    /// - `H(M)`: is the hash of the message being appended.
    /// This value is initialized to a special value that consists of all zeroes which indicates
    /// that no messages were previously added.
    mqc_head: Hash,
}
```
HRMP related storage layout

```rust,ignore
/// Pending HRMP open channel requests.
HrmpOpenChannelRequests: Vec<HrmpOpenChannelRequest>;
/// This mapping tracks how many open channel requests are inititated by a given sender para.
/// Invariant: `HrmpOpenChannelRequests` should contain the same number of items that has `_.sender == X`
/// as the number of `HrmpOpenChannelRequestCount` for `X`.
HrmpOpenChannelRequestCount: map ParaId => u32;
/// Pending HRMP close channel requests.
HrmpCloseChannelRequests: Vec<HrmpCloseChannelRequest>;
/// The HRMP watermark associated with each para.
HrmpWatermarks: map ParaId => Option<BlockNumber>;
/// HRMP channel data associated with each para.
HrmpChannels: map HrmpChannelId => Option<HrmpChannel>;
/// The indexes that map all senders to their recievers and vise versa.
/// Invariants:
/// - for each ingress index entry for `P` each item `I` in the index should present in `HrmpChannels` as `(I, P)`.
/// - for each egress index entry for `P` each item `E` in the index should present in `HrmpChannels` as `(P, E)`.
/// - there should be no other dangling channels in `HrmpChannels`.
HrmpIngressChannelsIndex: map ParaId => Vec<ParaId>;
HrmpEgressChannelsIndex: map ParaId => Vec<ParaId>;
/// Storage for the messages for each channel.
/// Invariant: cannot be non-empty if the corresponding channel in `HrmpChannels` is `None`.
HrmpChannelContents: map HrmpChannelId => Vec<InboundHrmpMessage>;
/// Maintains a mapping that can be used to answer the question:
/// What paras sent a message at the given block number for a given reciever.
HrmpChannelDigests: map ParaId => Vec<(BlockNumber, Vec<ParaId>)>;
```

## Initialization

No initialization routine runs for this module.

## Entry points

The following routines are intended to be invoked by paras' upward messages.

* `hrmp_init_open_channel(recipient)`:
  1. Check that the origin of this message is a para. We say that the origin of this message is the `sender`.
  1. Check that the `sender` is not `recipient`.
  1. Check that the `recipient` para exists.
  1. Check that there is no existing open channel request from sender to recipient.
  1. Check that the sum of the number of already opened HRMP channels by the `sender` (the size of the set found `HrmpEgressChannelsIndex` for `sender`) and the number of open requests by the `sender` (the value from `HrmpOpenChannelRequestCount` for `sender`) doesn't exceed the limit of channels (`config.hrmp_max_parachain_outbound_channels` or `config.hrmp_max_parathread_outbound_channels`) minus 1.
  1. Reserve the deposit for the `sender` according to `config.hrmp_sender_deposit`
  1. Add a new entry to `HrmpOpenChannelRequests` and increase `HrmpOpenChannelRequestCount` by 1 for the `sender`.
      1. Set `sender_deposit` to `config.hrmp_sender_deposit`
      1. Set `limit_used_places` to `config.hrmp_channel_max_places`
      1. Set `limit_limit_used_bytes` to `config.hrmp_channel_max_size`

* `hrmp_accept_open_channel(i)`, `i` - is the index of open channel request:
  1. Check that the designated open channel request exists
  1. Check that the request's `recipient` corresponds to the origin of this message.
  1. Reserve the deposit for the `recipient` according to `config.hrmp_recipient_deposit`
  1. Set the request's `confirmed` flag to `true`.

* `hrmp_close_channel(sender, recipient)`:
  1. Check that the channel between `sender` and `recipient` exists
  1. Check that the origin of the message is either `sender` or `recipient`
  1. Check that there is no existing intention to close the channel between `sender` and `recipient`.
  1. Add a new entry to `HrmpCloseChannelRequests` with initiator set to the `Some` variant with the origin of this message.

## Routines

Candidate Acceptance Function:

* `check_processed_downward_messages(P: ParaId, processed_downward_messages)`:
  1. Checks that `DownwardMessageQueues` for `P` is at least `processed_downward_messages` long.
  1. Checks that `processed_downward_messages` is at least 1 if `DownwardMessageQueues` for `P` is not empty.
* `check_hrmp_watermark(P: ParaId, new_hrmp_watermark)`:
  1. `new_hrmp_watermark` should be strictly greater than the value of `HrmpWatermarks` for `P` (if any).
  1. `new_hrmp_watermark` must not be greater than the context's block number.
  1. in `HrmpChannelDigests` for `P` an entry with the block number equal to `new_hrmp_watermark` should exist.
* `verify_outbound_hrmp(sender: ParaId, Vec<OutboundHrmpMessage>)`:
  1. For each horizontal message `M` with the channel `C` identified by `(sender, M.recipient)` check:
      1. exists
      1. `M`'s payload size summed with the `C.used_bytes` doesn't exceed a preconfigured limit `C.limit_used_bytes`.
      1. `C.used_places + 1` doesn't exceed a preconfigured limit `C.limit_used_places`.

Candidate Enactment:

* `queue_outbound_hrmp(sender: ParaId, Vec<OutboundHrmpMessage>)`:
  1. For each horizontal message `HM` with the channel `C` identified by `(sender, HM.recipient)`:
    1. Append `HM` into `HrmpChannelContents` that corresponds to `C`.
    1. Locate or create an entry in ``HrmpChannelDigests`` for `HM.recipient` and append `sender` into the entry's list.
    1. Increment `C.used_places`
    1. Increment `C.used_bytes` by `HM`'s payload size
    1. Append a new link to the MQC and save the new head in `C.mqc_head`. Note that the current block number as of enactment is used for the link.
* `prune_hrmp(recipient, new_hrmp_watermark)`:
  1. From ``HrmpChannelDigests`` for `recipient` remove all entries up to an entry with block number equal to `new_hrmp_watermark`.
  1. From the removed digests construct a set of paras that sent new messages within the interval between the old and new watermarks.
  1. For each channel `C` identified by `(sender, recipient)` for each `sender` coming from the set, prune messages up to the `new_hrmp_watermark`.
  1. For each pruned message `M` from channel `C`:
      1. Decrement `C.used_places`
      1. Decrement `C.used_bytes` by `M`'s payload size.
  1. Set `HrmpWatermarks` for `P` to be equal to `new_hrmp_watermark`
* `prune_dmq(P: ParaId, processed_downward_messages)`:
  1. Remove the first `processed_downward_messages` from the `DownwardMessageQueues` of `P`.

* `queue_upward_messages(ParaId, Vec<UpwardMessage>)`:
  1. Updates `NeedsDispatch`, and enqueues upward messages into `RelayDispatchQueue` and modifies the respective entry in `RelayDispatchQueueSize`.

The following routine is intended to be called in the same time when `Paras::schedule_para_cleanup` is called.

`schedule_para_cleanup(ParaId)`:
    1. Add the para into the `OutgoingParas` vector maintaining the sorted order.

## Session Change

1. Drain `OutgoingParas`. For each `P` happened to be in the list:
  1. Remove all inbound channels of `P`, i.e. `(_, P)`,
  1. Remove all outbound channels of `P`, i.e. `(P, _)`,
  1. Remove all `DownwardMessageQueues` of `P`.
  1. Remove `RelayDispatchQueueSize` of `P`.
  1. Remove `RelayDispatchQueues` of `P`.
  - Note that we don't remove the open/close requests since they are gon die out naturally.
TODO: What happens with the deposits in channels or open requests?
1. For each request `R` in `HrmpOpenChannelRequests`:
    1. if `R.confirmed = false`:
        1. increment `R.age` by 1.
        1. if `R.age` reached a preconfigured time-to-live limit `config.hrmp_open_request_ttl`, then:
            1. refund `R.sender_deposit` to the sender
            1. decrement `HrmpOpenChannelRequestCount` for `R.sender` by 1.
            1. remove `R`
    2. if `R.confirmed = true`,
        1. check that `R.sender` and `R.recipient` are not offboarded.
        1. create a new channel `C` between `(R.sender, R.recipient)`.
            1. Initialize the `C.sender_deposit` with `R.sender_deposit` and `C.recipient_deposit` with the value found in the configuration `config.hrmp_recipient_deposit`.
            1. Insert `sender` into the set `HrmpIngressChannelsIndex` for the `recipient`.
            1. Insert `recipient` into the set `HrmpEgressChannelsIndex` for the `sender`.
        1. decrement `HrmpOpenChannelRequestCount` for `R.sender` by 1.
        1. remove `R`
1. For each request `R` in `HrmpCloseChannelRequests` remove the channel identified by `R.id`, if exists.

To remove a channel `C` identified with a tuple `(sender, recipient)`:

1. Return `C.sender_deposit` to the `sender`.
1. Return `C.recipient_deposit` to the `recipient`.
1. Remove `C` from `HrmpChannels`.
1. Remove `C` from `HrmpChannelContents`.
1. Remove `recipient` from the set `HrmpEgressChannelsIndex` for `sender`.
1. Remove `sender` from the set `HrmpIngressChannelsIndex` for `recipient`.

## Finalization

  1. Dispatch queued upward messages from `RelayDispatchQueues` in a FIFO order applying the `config.watermark_upward_queue_size` and `config.max_upward_queue_count` limits.
