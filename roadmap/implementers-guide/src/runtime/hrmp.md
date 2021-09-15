# HRMP Module

A module responsible for Horizontally Relay-routed Message Passing (HRMP). See [Messaging Overview](../messaging.md) for more details.

## Storage

HRMP related structs:

```rust
/// A description of a request to open an HRMP channel.
struct HrmpOpenChannelRequest {
    /// Indicates if this request was confirmed by the recipient.
    confirmed: bool,
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
    > NOTE: That collecting digests can be inefficient and the time it takes grows very fast. Thanks to the aggressive
    > parameterization this shouldn't be a big of a deal.
    > If that becomes a problem consider introducing an extra dictionary which says at what block the given sender
    > sent a message to the recipient.

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
* `hrmp_cancel_open_request(ch)`:
    1. Check that `origin` is either `ch.sender` or `ch.recipient`
    1. Check that the open channel request `ch` exists.
    1. Check that the open channel request for `ch` is not confirmed.
    1. Remove `ch` from `HrmpOpenChannelRequests` and `HrmpOpenChannelRequestsList`
    1. Decrement `HrmpAcceptedChannelRequestCount` for `ch.recipient` by 1.
    1. Unreserve the deposit of `ch.sender`.
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

1. For each `P` in `outgoing_paras` (generated by `Paras::on_new_session`):
    1. Remove all inbound channels of `P`, i.e. `(_, P)`,
    1. Remove all outbound channels of `P`, i.e. `(P, _)`,
    1. Remove `HrmpOpenChannelRequestCount` for `P`
    1. Remove `HrmpAcceptedChannelRequestCount` for `P`.
    1. Remove `HrmpOpenChannelRequests` and `HrmpOpenChannelRequestsList` for `(P, _)` and `(_, P)`.
        1. For each removed channel request `C`:
            1. Unreserve the sender's deposit if the sender is not present in `outgoing_paras`
            1. Unreserve the recipient's deposit if `C` is confirmed and the recipient is not present in `outgoing_paras`
1. For each channel designator `D` in `HrmpOpenChannelRequestsList` we query the request `R` from `HrmpOpenChannelRequests`:
    1. if `R.confirmed = true`,
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
