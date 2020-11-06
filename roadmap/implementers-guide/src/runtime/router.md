# Router Module

The Router module is responsible for all messaging mechanisms supported between paras and the relay chain, specifically: UMP, DMP, HRMP and later XCMP.

## Storage

General storage entries

```rust
/// Paras that are to be cleaned up at the end of the session.
/// The entries are sorted ascending by the para id.
OutgoingParas: Vec<ParaId>;
```

### Upward Message Passing (UMP)

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

### Downward Message Passing (DMP)

Storage layout required for implementation of DMP.

```rust
/// The downward messages addressed for a certain para.
DownwardMessageQueues: map ParaId => Vec<InboundDownwardMessage>;
/// A mapping that stores the downward message queue MQC head for each para.
///
/// Each link in this chain has a form:
/// `(prev_head, B, H(M))`, where
/// - `prev_head`: is the previous head hash or zero if none.
/// - `B`: is the relay-chain block number in which a message was appended.
/// - `H(M)`: is the hash of the message being appended.
DownwardMessageQueueHeads: map ParaId => Hash;
```

### HRMP

HRMP related structs:

```rust
/// A description of a request to open an HRMP channel.
struct HrmpOpenChannelRequest {
    /// Indicates if this request was confirmed by the recipient.
    confirmed: bool,
    /// How many session boundaries ago this request was seen.
    age: SessionIndex,
    /// The amount that the sender supplied at the time of creation of this request.
    sender_deposit: Balance,
    /// The maximum message size that could be put into the channel.
    max_message_size: u32,
    /// The maximum number of messages that can be pending in the channel at once.
    max_capacity: u32,
    /// The maximum total size of the messages that can be pending in the channel at once.
    max_total_size: u32,
}

/// A metadata of an HRMP channel.
struct HrmpChannel {
    /// The amount that the sender supplied as a deposit when opening this channel.
    sender_deposit: Balance,
    /// The amount that the recipient supplied as a deposit when accepting opening this channel.
    recipient_deposit: Balance,
    /// The maximum number of messages that can be pending in the channel at once.
    max_capacity: u32,
    /// The maximum total size of the messages that can be pending in the channel at once.
    max_total_size: u32,
    /// The maximum message size that could be put into the channel.
    max_message_size: u32,
    /// The current number of messages pending in the channel.
    /// Invariant: should be less or equal to `max_capacity`.
    msg_count: u32,
    /// The total size in bytes of all message payloads in the channel.
    /// Invariant: should be less or equal to `max_total_size`.
    total_size: u32,
    /// A head of the Message Queue Chain for this channel. Each link in this chain has a form:
    /// `(prev_head, B, H(M))`, where
    /// - `prev_head`: is the previous value of `mqc_head` or zero if none.
    /// - `B`: is the [relay-chain] block number in which a message was appended
    /// - `H(M)`: is the hash of the message being appended.
    /// This value is initialized to a special value that consists of all zeroes which indicates
    /// that no messages were previously added.
    mqc_head: Option<Hash>,
}
```
HRMP related storage layout

```rust
/// The set of pending HRMP open channel requests.
///
/// The set is accompanied by a list for iteration.
///
/// Invariant:
/// - There are no channels that exists in list but not in the set and vice versa.
HrmpOpenChannelRequests: map HrmpChannelId => Option<HrmpOpenChannelRequest>;
HrmpOpenChannelRequestsList: Vec<HrmpChannelId>;

/// This mapping tracks how many open channel requests are inititated by a given sender para.
/// Invariant: `HrmpOpenChannelRequests` should contain the same number of items that has `(X, _)`
/// as the number of `HrmpOpenChannelRequestCount` for `X`.
HrmpOpenChannelRequestCount: map ParaId => u32;
/// This mapping tracks how many open channel requests were accepted by a given recipient para.
/// Invariant: `HrmpOpenChannelRequests` should contain the same number of items `(_, X)` with
/// `confirmed` set to true, as the number of `HrmpAcceptedChannelRequestCount` for `X`.
HrmpAcceptedChannelRequestCount: map ParaId => u32;

/// A set of pending HRMP close channel requests that are going to be closed during the session change.
/// Used for checking if a given channel is registered for closure.
///
/// The set is accompanied by a list for iteration.
///
/// Invariant:
/// - There are no channels that exists in list but not in the set and vice versa.
HrmpCloseChannelRequests: map HrmpChannelId => Option<()>;
HrmpCloseChannelRequestsList: Vec<HrmpChannelId>;

/// The HRMP watermark associated with each para.
/// Invariant:
/// - each para `P` used here as a key should satisfy `Paras::is_valid_para(P)` within a session.
HrmpWatermarks: map ParaId => Option<BlockNumber>;
/// HRMP channel data associated with each para.
/// Invariant:
/// - each participant in the channel should satisfy `Paras::is_valid_para(P)` within a session.
HrmpChannels: map HrmpChannelId => Option<HrmpChannel>;
/// Ingress/egress indexes allow to find all the senders and receivers given the opposite
/// side. I.e.
///
/// (a) ingress index allows to find all the senders for a given recipient.
/// (b) egress index allows to find all the recipients for a given sender.
///
/// Invariants:
/// - for each ingress index entry for `P` each item `I` in the index should present in `HrmpChannels`
///   as `(I, P)`.
/// - for each egress index entry for `P` each item `E` in the index should present in `HrmpChannels`
///   as `(P, E)`.
/// - there should be no other dangling channels in `HrmpChannels`.
/// - the vectors are sorted.
HrmpIngressChannelsIndex: map ParaId => Vec<ParaId>;
HrmpEgressChannelsIndex: map ParaId => Vec<ParaId>;
/// Storage for the messages for each channel.
/// Invariant: cannot be non-empty if the corresponding channel in `HrmpChannels` is `None`.
HrmpChannelContents: map HrmpChannelId => Vec<InboundHrmpMessage>;
/// Maintains a mapping that can be used to answer the question:
/// What paras sent a message at the given block number for a given reciever.
/// Invariants:
/// - The inner `Vec<ParaId>` is never empty.
/// - The inner `Vec<ParaId>` cannot store two same `ParaId`.
/// - The outer vector is sorted ascending by block number and cannot store two items with the same
///   block number.
HrmpChannelDigests: map ParaId => Vec<(BlockNumber, Vec<ParaId>)>;
```

## Initialization

No initialization routine runs for this module.

## Routines

Candidate Acceptance Function:

* `check_upward_messages(P: ParaId, Vec<UpwardMessage>`):
    1. Checks that there are at most `config.max_upward_message_num_per_candidate` messages.
    1. Checks that no message exceeds `config.max_upward_message_size`.
    1. Verify that `RelayDispatchQueueSize` for `P` has enough capacity for the messages
* `check_processed_downward_messages(P: ParaId, processed_downward_messages)`:
    1. Checks that `DownwardMessageQueues` for `P` is at least `processed_downward_messages` long.
    1. Checks that `processed_downward_messages` is at least 1 if `DownwardMessageQueues` for `P` is not empty.
* `check_hrmp_watermark(P: ParaId, new_hrmp_watermark)`:
    1. `new_hrmp_watermark` should be strictly greater than the value of `HrmpWatermarks` for `P` (if any).
    1. `new_hrmp_watermark` must not be greater than the context's block number.
    1. `new_hrmp_watermark` should be either
        1. equal to the context's block number
        1. or in `HrmpChannelDigests` for `P` an entry with the block number should exist
* `check_outbound_hrmp(sender: ParaId, Vec<OutboundHrmpMessage>)`:
    1. Checks that there are at most `config.hrmp_max_message_num_per_candidate` messages.
    1. Checks that horizontal messages are sorted by ascending recipient ParaId and there is no two horizontal messages have the same recipient.
    1. For each horizontal message `M` with the channel `C` identified by `(sender, M.recipient)` check:
        1. exists
        1. `M`'s payload size doesn't exceed a preconfigured limit `C.max_message_size`
        1. `M`'s payload size summed with the `C.total_size` doesn't exceed a preconfigured limit `C.max_total_size`.
        1. `C.msg_count + 1` doesn't exceed a preconfigured limit `C.max_capacity`.

Candidate Enactment:

* `queue_outbound_hrmp(sender: ParaId, Vec<OutboundHrmpMessage>)`:
    1. For each horizontal message `HM` with the channel `C` identified by `(sender, HM.recipient)`:
        1. Append `HM` into `HrmpChannelContents` that corresponds to `C` with `sent_at` equals to the current block number.
        1. Locate or create an entry in `HrmpChannelDigests` for `HM.recipient` and append `sender` into the entry's list.
        1. Increment `C.msg_count`
        1. Increment `C.total_size` by `HM`'s payload size
        1. Append a new link to the MQC and save the new head in `C.mqc_head`. Note that the current block number as of enactment is used for the link.
* `prune_hrmp(recipient, new_hrmp_watermark)`:
    1. From `HrmpChannelDigests` for `recipient` remove all entries up to an entry with block number equal to `new_hrmp_watermark`.
    1. From the removed digests construct a set of paras that sent new messages within the interval between the old and new watermarks.
    1. For each channel `C` identified by `(sender, recipient)` for each `sender` coming from the set, prune messages up to the `new_hrmp_watermark`.
    1. For each pruned message `M` from channel `C`:
        1. Decrement `C.msg_count`
        1. Decrement `C.total_size` by `M`'s payload size.
    1. Set `HrmpWatermarks` for `P` to be equal to `new_hrmp_watermark`
    > NOTE: That collecting digests can be inefficient and the time it takes grows very fast. Thanks to the aggresive
    > parametrization this shouldn't be a big of a deal.
    > If that becomes a problem consider introducing an extra dictionary which says at what block the given sender
    > sent a message to the recipient.
* `prune_dmq(P: ParaId, processed_downward_messages)`:
    1. Remove the first `processed_downward_messages` from the `DownwardMessageQueues` of `P`.
* `enact_upward_messages(P: ParaId, Vec<UpwardMessage>)`:
    1. Process each upward message `M` in order:
        1. Append the message to `RelayDispatchQueues` for `P`
        1. Increment the size and the count in `RelayDispatchQueueSize` for `P`.
        1. Ensure that `P` is present in `NeedsDispatch`.

The following routine is intended to be called in the same time when `Paras::schedule_para_cleanup` is called.

`schedule_para_cleanup(ParaId)`:
    1. Add the para into the `OutgoingParas` vector maintaining the sorted order.

The following routine is meant to execute pending entries in upward message queues. This function doesn't fail, even if
dispatcing any of individual upward messages returns an error.

`process_pending_upward_messages()`:
    1. Initialize a cumulative weight counter `T` to 0
    1. Iterate over items in `NeedsDispatch` cyclically, starting with `NextDispatchRoundStartWith`. If the item specified is `None` start from the beginning. For each `P` encountered:
        1. Dequeue the first upward message `D` from `RelayDispatchQueues` for `P`
        1. Decrement the size of the message from `RelayDispatchQueueSize` for `P`
        1. Delegate processing of the message to the runtime. The weight consumed is added to `T`.
        1. If `T >= config.preferred_dispatchable_upward_messages_step_weight`, set `NextDispatchRoundStartWith` to `P` and finish processing.
        1. If `RelayDispatchQueues` for `P` became empty, remove `P` from `NeedsDispatch`.
        1. If `NeedsDispatch` became empty then finish processing and set `NextDispatchRoundStartWith` to `None`.
        > NOTE that in practice we would need to approach the weight calculation more thoroughly, i.e. incorporate all operations
        > that could take place on the course of handling these upward messages.

Utility routines.

`queue_downward_message(P: ParaId, M: DownwardMessage)`:
    1. Check if the size of `M` exceeds the `config.max_downward_message_size`. If so, return an error.
    1. Wrap `M` into `InboundDownwardMessage` using the current block number for `sent_at`.
    1. Obtain a new MQC link for the resulting `InboundDownwardMessage` and replace `DownwardMessageQueueHeads` for `P` with the resulting hash.
    1. Add the resulting `InboundDownwardMessage` into `DownwardMessageQueues` for `P`.

## Entry-points

The following entry-points are meant to be used for HRMP channel management.

Those entry-points are meant to be called from a parachain. `origin` is defined as the `ParaId` of
the parachain executed the message.

* `hrmp_init_open_channel(recipient, proposed_max_capacity, proposed_max_message_size)`:
    1. Check that the `origin` is not `recipient`.
    1. Check that `proposed_max_capacity` is less or equal to `config.hrmp_channel_max_capacity` and greater than zero.
    1. Check that `proposed_max_message_size` is less or equal to `config.hrmp_channel_max_message_size` and greater than zero.
    1. Check that `recipient` is a valid para.
    1. Check that there is no existing channel for `(origin, recipient)` in `HrmpChannels`.
    1. Check that there is no existing open channel request (`origin`, `recipient`) in `HrmpOpenChannelRequests`.
    1. Check that the sum of the number of already opened HRMP channels by the `origin` (the size
    of the set found `HrmpEgressChannelsIndex` for `origin`) and the number of open requests by the
    `origin` (the value from `HrmpOpenChannelRequestCount` for `origin`) doesn't exceed the limit of
    channels (`config.hrmp_max_parachain_outbound_channels` or `config.hrmp_max_parathread_outbound_channels`) minus 1.
    1. Check that `origin`'s balance is more or equal to `config.hrmp_sender_deposit`
    1. Reserve the deposit for the `origin` according to `config.hrmp_sender_deposit`
    1. Increase `HrmpOpenChannelRequestCount` by 1 for `origin`.
    1. Append `(origin, recipient)` to `HrmpOpenChannelRequestsList`.
    1. Add a new entry to `HrmpOpenChannelRequests` for `(origin, recipient)`
        1. Set `sender_deposit` to `config.hrmp_sender_deposit`
        1. Set `max_capacity` to `proposed_max_capacity`
        1. Set `max_message_size` to `proposed_max_message_size`
        1. Set `max_total_size` to `config.hrmp_channel_max_total_size`
    1. Send a downward message to `recipient` notifying about an inbound HRMP channel request.
        - The DM is sent using `queue_downward_message`.
        - The DM is represented by the `HrmpNewChannelOpenRequest`  XCM message.
            - `sender` is set to `origin`,
            - `max_message_size` is set to `proposed_max_message_size`,
            - `max_capacity` is set to `proposed_max_capacity`.
* `hrmp_accept_open_channel(sender)`:
    1. Check that there is an existing request between (`sender`, `origin`) in `HrmpOpenChannelRequests`
        1. Check that it is not confirmed.
    1. Check that the sum of the number of inbound HRMP channels opened to `origin` (the size of the set
    found in `HrmpIngressChannelsIndex` for `origin`) and the number of accepted open requests by the `origin`
    (the value from `HrmpAcceptedChannelRequestCount` for `origin`) doesn't exceed the limit of channels
    (`config.hrmp_max_parachain_inbound_channels` or `config.hrmp_max_parathread_inbound_channels`)
    minus 1.
    1. Check that `origin`'s balance is more or equal to `config.hrmp_recipient_deposit`.
    1. Reserve the deposit for the `origin` according to `config.hrmp_recipient_deposit`
    1. For the request in `HrmpOpenChannelRequests` identified by `(sender, P)`, set `confirmed` flag to `true`.
    1. Increase `HrmpAcceptedChannelRequestCount` by 1 for `origin`.
    1. Send a downward message to `sender` notifying that the channel request was accepted.
        - The DM is sent using `queue_downward_message`.
        - The DM is represented by the `HrmpChannelAccepted` XCM message.
            - `recipient` is set to `origin`.
* `hrmp_close_channel(ch)`:
    1. Check that `origin` is either `ch.sender` or `ch.recipient`
    1. Check that `HrmpChannels` for `ch` exists.
    1. Check that `ch` is not in the `HrmpCloseChannelRequests` set.
    1. If not already there, insert a new entry `Some(())` to `HrmpCloseChannelRequests` for `ch`
    and append `ch` to `HrmpCloseChannelRequestsList`.
    1. Send a downward message to the opposite party notifying about the channel closing.
        - The DM is sent using `queue_downward_message`.
        - The DM is represented by the `HrmpChannelClosing` XCM message with:
            - `initator` is set to `origin`,
            - `sender` is set to `ch.sender`,
            - `recipient` is set to `ch.recipient`.
        - The opposite party is `ch.sender` if `origin` is `ch.recipient` and `ch.recipient` if `origin` is `ch.sender`.

## Session Change

1. Drain `OutgoingParas`. For each `P` happened to be in the list:
    1. Remove all inbound channels of `P`, i.e. `(_, P)`,
    1. Remove all outbound channels of `P`, i.e. `(P, _)`,
    1. Remove all `DownwardMessageQueues` of `P`.
    1. Remove `DownwardMessageQueueHeads` for `P`.
    1. Remove `RelayDispatchQueueSize` of `P`.
    1. Remove `RelayDispatchQueues` of `P`.
    1. Remove `HrmpOpenChannelRequestCount` for `P`
    1. Remove `HrmpAcceptedChannelRequestCount` for `P`.
    1. Remove `P` if it exists in `NeedsDispatch`.
    1. If `P` is in `NextDispatchRoundStartWith`, then reset it to `None`
    - Note that if we don't remove the open/close requests since they are going to die out naturally at the end of the session.
1. For each channel designator `D` in `HrmpOpenChannelRequestsList` we query the request `R` from `HrmpOpenChannelRequests`:
    1. if `R.confirmed = false`:
        1. increment `R.age` by 1.
        1. if `R.age` reached a preconfigured time-to-live limit `config.hrmp_open_request_ttl`, then:
            1. refund `R.sender_deposit` to the sender
            1. decrement `HrmpOpenChannelRequestCount` for `D.sender` by 1.
            1. remove `R`
            1. remove `D`
    2. if `R.confirmed = true`,
        1. if both `D.sender` and `D.recipient` are not offboarded.
          1. create a new channel `C` between `(D.sender, D.recipient)`.
              1. Initialize the `C.sender_deposit` with `R.sender_deposit` and `C.recipient_deposit`
              with the value found in the configuration `config.hrmp_recipient_deposit`.
              1. Insert `sender` into the set `HrmpIngressChannelsIndex` for the `recipient`.
              1. Insert `recipient` into the set `HrmpEgressChannelsIndex` for the `sender`.
        1. decrement `HrmpOpenChannelRequestCount` for `D.sender` by 1.
        1. decrement `HrmpAcceptedChannelRequestCount` for `D.recipient` by 1.
        1. remove `R`
        1. remove `D`
1. For each HRMP channel designator `D` in `HrmpCloseChannelRequestsList`
    1. remove the channel identified by `D`, if exists.
    1. remove `D` from `HrmpCloseChannelRequests`.
    1. remove `D` from `HrmpCloseChannelRequestsList`

To remove a HRMP channel `C` identified with a tuple `(sender, recipient)`:

1. Return `C.sender_deposit` to the `sender`.
1. Return `C.recipient_deposit` to the `recipient`.
1. Remove `C` from `HrmpChannels`.
1. Remove `C` from `HrmpChannelContents`.
1. Remove `recipient` from the set `HrmpEgressChannelsIndex` for `sender`.
1. Remove `sender` from the set `HrmpIngressChannelsIndex` for `recipient`.
