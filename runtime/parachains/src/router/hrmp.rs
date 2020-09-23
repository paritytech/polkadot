// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use super::{Module, Store, Trait};
use crate::{configuration::HostConfiguration, paras};
use codec::{Decode, Encode};
use frame_support::{traits::Get, weights::Weight, StorageMap, StorageValue};
use primitives::v1::{
	Balance, Hash, HrmpChannelId, Id as ParaId, InboundHrmpMessage, OutboundHrmpMessage,
	SessionIndex,
};
use sp_runtime::traits::{BlakeTwo256, Hash as HashT};
use sp_std::collections::btree_set::BTreeSet;
use sp_std::mem;
use sp_std::prelude::*;

/// A description of a request to open an HRMP channel.
#[derive(Encode, Decode)]
pub struct HrmpOpenChannelRequest {
	/// Indicates if this request was confirmed by the recipient.
	pub confirmed: bool,
	/// How many session boundaries ago this request was seen.
	pub age: SessionIndex,
	/// The amount that the sender supplied at the time of creation of this request.
	pub sender_deposit: Balance,
	/// The maximum message size that could be put into the channel.
	pub max_message_size: u32,
	/// The maximum number of messages that can be pending in the channel at once.
	pub max_capacity: u32,
	/// The maximum total size of the messages that can be pending in the channel at once.
	pub max_total_size: u32,
}

/// A metadata of an HRMP channel.
#[derive(Encode, Decode)]
pub struct HrmpChannel {
	/// The amount that the sender supplied as a deposit when opening this channel.
	pub sender_deposit: Balance,
	/// The amount that the recipient supplied as a deposit when accepting opening this channel.
	pub recipient_deposit: Balance,
	/// The maximum number of messages that can be pending in the channel at once.
	pub max_capacity: u32,
	/// The maximum total size of the messages that can be pending in the channel at once.
	pub max_total_size: u32,
	/// The maximum message size that could be put into the channel.
	pub max_message_size: u32,
	/// The current number of messages pending in the channel.
    /// Invariant: should be less or equal to `max_capacity`.s`.
	pub msg_count: u32,
	/// The total size in bytes of all message payloads in the channel.
    /// Invariant: should be less or equal to `max_total_size`.
	pub total_size: u32,
	/// A head of the Message Queue Chain for this channel. Each link in this chain has a form:
	/// `(prev_head, B, H(M))`, where
	/// - `prev_head`: is the previous value of `mqc_head` or zero if none.
	/// - `B`: is the [relay-chain] block number in which a message was appended
	/// - `H(M)`: is the hash of the message being appended.
	/// This value is initialized to a special value that consists of all zeroes which indicates
	/// that no messages were previously added.
	pub mqc_head: Option<Hash>,
}

/// Routines and getters related to HRMP.
impl<T: Trait> Module<T> {
	/// Remove all storage entries associated with the given para.
	pub(super) fn clean_hrmp_after_outgoing(outgoing_para: ParaId) {
		<Self as Store>::HrmpOpenChannelRequestCount::remove(&outgoing_para);
		<Self as Store>::HrmpAcceptedChannelRequestCount::remove(&outgoing_para);

		// close all channels where the outgoing para acts as the recipient.
		for sender in <Self as Store>::HrmpIngressChannelsIndex::take(&outgoing_para) {
			Self::close_hrmp_channel(&HrmpChannelId {
				sender,
				recipient: outgoing_para.clone(),
			});
		}
		// close all channels where the outgoing para acts as the sender.
		for recipient in <Self as Store>::HrmpEgressChannelsIndex::take(&outgoing_para) {
			Self::close_hrmp_channel(&HrmpChannelId {
				sender: outgoing_para.clone(),
				recipient,
			});
		}
	}

	/// Iterate over all open channel requests and:
	///
	/// - prune the stale requests
	/// - enact the confirmed requests
	pub(super) fn process_hrmp_open_channel_requests(config: &HostConfiguration<T::BlockNumber>) {
		let mut open_req_channels = <Self as Store>::HrmpOpenChannelRequestsList::get();
		if open_req_channels.is_empty() {
			return;
		}

		// iterate the vector starting from the end making our way to the beginning. This way we
		// can leverage `swap_remove` to efficiently remove an item during iteration.
		let mut idx = open_req_channels.len();
		loop {
			// bail if we've iterated over all items.
			if idx == 0 {
				break;
			}

			idx -= 1;
			let channel_id = open_req_channels[idx].clone();
			let mut request = <Self as Store>::HrmpOpenChannelRequests::get(&channel_id)
				.expect(
					"can't be `None` due to the invariant that the list contains the same items as the set; qed"
				);

			if request.confirmed {
				if <paras::Module<T>>::is_valid_para(channel_id.sender)
					&& <paras::Module<T>>::is_valid_para(channel_id.sender)
				{
					<Self as Store>::HrmpChannels::insert(
						&channel_id,
						HrmpChannel {
							sender_deposit: request.sender_deposit,
							recipient_deposit: config.hrmp_recipient_deposit,
							max_capacity: request.max_capacity,
							max_total_size: request.max_total_size,
							max_message_size: request.max_message_size,
							msg_count: 0,
							total_size: 0,
							mqc_head: None,
						},
					);

					<Self as Store>::HrmpIngressChannelsIndex::mutate(&channel_id.recipient, |v| {
						if let Err(i) = v.binary_search(&channel_id.sender) {
							v.insert(i, channel_id.sender);
						}
					});
					<Self as Store>::HrmpEgressChannelsIndex::mutate(&channel_id.sender, |v| {
						if let Err(i) = v.binary_search(&channel_id.recipient) {
							v.insert(i, channel_id.recipient);
						}
					});
				}

				<Self as Store>::HrmpOpenChannelRequestCount::mutate(&channel_id.sender, |v| {
					*v -= 1;
				});
				<Self as Store>::HrmpAcceptedChannelRequestCount::mutate(
					&channel_id.recipient,
					|v| {
						*v -= 1;
					},
				);

				let _ = open_req_channels.swap_remove(idx);
				<Self as Store>::HrmpOpenChannelRequests::remove(&channel_id);
			} else {
				request.age += 1;
				if request.age == config.hrmp_open_request_ttl {
					// got stale

					<Self as Store>::HrmpOpenChannelRequestCount::mutate(&channel_id.sender, |v| {
						*v -= 1;
					});

					// TODO: return deposit

					let _ = open_req_channels.swap_remove(idx);
					<Self as Store>::HrmpOpenChannelRequests::remove(&channel_id);
				}
			}
		}

		<Self as Store>::HrmpOpenChannelRequestsList::put(open_req_channels);
	}

	/// Iterate over all close channel requests unconditionally closing the channels.
	pub(super) fn process_hrmp_close_channel_requests() {
		let close_reqs = <Self as Store>::HrmpCloseChannelRequestsList::take();
		for condemned_ch_id in close_reqs {
			<Self as Store>::HrmpCloseChannelRequests::remove(&condemned_ch_id);
			Self::close_hrmp_channel(&condemned_ch_id);

			// clean up the indexes.
			<Self as Store>::HrmpEgressChannelsIndex::mutate(&condemned_ch_id.sender, |v| {
				if let Ok(i) = v.binary_search(&condemned_ch_id.recipient) {
					v.remove(i);
				}
			});
			<Self as Store>::HrmpIngressChannelsIndex::mutate(&condemned_ch_id.recipient, |v| {
				if let Ok(i) = v.binary_search(&condemned_ch_id.sender) {
					v.remove(i);
				}
			});
		}
	}

	/// Close and remove the designated HRMP channel.
	///
	/// This includes returning the deposits. However, it doesn't include updating the ingress/egress
	/// indicies.
	pub(super) fn close_hrmp_channel(channel_id: &HrmpChannelId) {
		// TODO: return deposits

		<Self as Store>::HrmpChannels::remove(channel_id);
		<Self as Store>::HrmpChannelContents::remove(channel_id);
	}

	/// Check that the candidate of the given recipient controls the HRMP watermark properly.
	pub(crate) fn check_hrmp_watermark(
		recipient: ParaId,
		relay_chain_parent_number: T::BlockNumber,
		new_hrmp_watermark: T::BlockNumber,
	) -> bool {
		// First, check where the watermark CANNOT legally land.
		//
		// (a) For ensuring that messages are eventually, a rule requires each parablock new
		//     watermark should be greater than the last one.
		//
		// (b) However, a parachain cannot read into "the future", therefore the watermark should
		//     not be greater than the relay-chain context block which the parablock refers to.
		if let Some(last_watermark) = <Self as Store>::HrmpWatermarks::get(&recipient) {
			if new_hrmp_watermark <= last_watermark {
				return false;
			}
		}
		if new_hrmp_watermark > relay_chain_parent_number {
			return false;
		}

		// Second, check where the watermark CAN land. It's one of the following:
		//
		// (a) The relay parent block number.
		// (b) A relay-chain block in which this para received at least one message.
		if new_hrmp_watermark == relay_chain_parent_number {
			true
		} else {
			let digest = <Self as Store>::HrmpChannelDigests::get(&recipient);
			digest
				.binary_search_by_key(&new_hrmp_watermark, |(block_no, _)| *block_no)
				.is_ok()
		}
	}

	pub(crate) fn check_outbound_hrmp(
		config: &HostConfiguration<T::BlockNumber>,
		sender: ParaId,
		out_hrmp_msgs: &[OutboundHrmpMessage<ParaId>],
	) -> bool {
		if out_hrmp_msgs.len() as u32 > config.hrmp_max_message_num_per_candidate {
			return false;
		}

		let mut last_recipient = None::<ParaId>;

		for out_msg in out_hrmp_msgs {
			match last_recipient {
				// the messages must be sorted in ascending order and there must be no two messages sent
				// to the same recipient. Thus we can check that every recipient is strictly greater than
				// the previous one.
				Some(last_recipient) if out_msg.recipient <= last_recipient => {
					return false;
				}
				_ => last_recipient = Some(out_msg.recipient),
			}

			let channel_id = HrmpChannelId {
				sender,
				recipient: out_msg.recipient,
			};

			let channel = match <Self as Store>::HrmpChannels::get(&channel_id) {
				Some(channel) => channel,
				None => return false,
			};

			if out_msg.data.len() as u32 > channel.max_message_size {
				return false;
			}

			if channel.total_size + out_msg.data.len() as u32 > channel.max_total_size {
				return false;
			}

			if channel.msg_count + 1 > channel.max_capacity {
				return false;
			}
		}

		true
	}

	pub(crate) fn prune_hrmp(recipient: ParaId, new_hrmp_watermark: T::BlockNumber) -> Weight {
		let mut weight = 0;

		// sift through the incoming messages digest to collect the paras that sent at least one
		// message to this parachain between the old and new watermarks.
		let senders = <Self as Store>::HrmpChannelDigests::mutate(&recipient, |digest| {
			let mut senders = BTreeSet::new();
			let mut residue = Vec::with_capacity(digest.len());
			for (block_no, paras_sent_msg) in mem::replace(digest, Vec::new()) {
				if block_no <= new_hrmp_watermark {
					senders.extend(paras_sent_msg);
				} else {
					residue.push((block_no, paras_sent_msg));
				}
			}
			*digest = residue;
			senders
		});
		weight += T::DbWeight::get().reads_writes(1, 1);

		// having all senders we can trivially find out the channels which we need to prune.
		let channels_to_prune = senders
			.into_iter()
			.map(|sender| HrmpChannelId { sender, recipient });
		for channel_id in channels_to_prune {
			// prune each channel up to the new watermark keeping track how many messages we removed
			// and what is the total byte size of them.
			let (cnt, size) =
				<Self as Store>::HrmpChannelContents::mutate(&channel_id, |outbound_messages| {
					let (mut cnt, mut size) = (0, 0);

					let mut residue = Vec::with_capacity(outbound_messages.len());
					for msg in mem::replace(outbound_messages, Vec::new()).into_iter() {
						if msg.sent_at <= new_hrmp_watermark {
							cnt += 1;
							size += msg.data.len();
						} else {
							residue.push(msg);
						}
					}
					*outbound_messages = residue;

					(cnt, size)
				});

			// update the channel metadata.
			<Self as Store>::HrmpChannels::mutate(&channel_id, |channel| {
				if let Some(ref mut channel) = channel {
					channel.msg_count -= cnt as u32;
					channel.total_size -= size as u32;
				}
			});

			weight += T::DbWeight::get().reads_writes(2, 2);
		}

		<Self as Store>::HrmpWatermarks::insert(&recipient, new_hrmp_watermark);
		weight += T::DbWeight::get().reads_writes(0, 1);

		weight
	}

	/// Process the outbound HRMP messages by putting them into the appropriate recipient queues.
	///
	/// Returns the amount of weight consumed.
	pub(crate) fn queue_outbound_hrmp(
		sender: ParaId,
		out_hrmp_msgs: Vec<OutboundHrmpMessage<ParaId>>,
	) -> Weight {
		let mut weight = 0;
		let now = <frame_system::Module<T>>::block_number();

		for out_msg in out_hrmp_msgs {
			let channel_id = HrmpChannelId {
				sender,
				recipient: out_msg.recipient,
			};

			let mut channel = match <Self as Store>::HrmpChannels::get(&channel_id) {
				Some(channel) => channel,
				None => {
					// apparently, that since acceptance of this candidate the recipient was
					// offboarded and the channel no longer exists.
					continue;
				}
			};

			let inbound = InboundHrmpMessage {
				sent_at: now,
				data: out_msg.data,
			};

			// book keeping
			channel.msg_count += 1;
			channel.total_size += inbound.data.len() as u32;

			// compute the new MQC head of the channel
			let prev_head = channel.mqc_head.clone().unwrap_or(Default::default());
			let new_head = BlakeTwo256::hash_of(&(
				prev_head,
				inbound.sent_at,
				T::Hashing::hash_of(&inbound.data),
			));
			channel.mqc_head = Some(new_head);

			<Self as Store>::HrmpChannels::insert(&channel_id, channel);
			<Self as Store>::HrmpChannelContents::append(&channel_id, inbound);

			weight += T::DbWeight::get().reads_writes(2, 2);
		}

		weight
	}

	pub(crate) fn hrmp_mqc_heads(recipient: ParaId) -> Vec<(ParaId, Hash)> {
		let sender_set = <Self as Store>::HrmpIngressChannelsIndex::get(&recipient);
		if sender_set.is_empty() {
			return Vec::new();
		}

		let mut mqc_heads = Vec::with_capacity(sender_set.len());
		for sender in sender_set {
			let channel_metadata =
				<Self as Store>::HrmpChannels::get(&HrmpChannelId { sender, recipient });
			let mqc_head = channel_metadata
				.and_then(|metadata| metadata.mqc_head)
				.unwrap_or(Hash::default());
			mqc_heads.push((sender, mqc_head));
		}

		mqc_heads
	}
}
