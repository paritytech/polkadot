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

use super::{Module, Store, Trait, Error as DispatchError, dmp};
use crate::{
	configuration::{self, HostConfiguration},
	paras,
};
use codec::{Decode, Encode};
use frame_support::{
	traits::Get, weights::Weight, StorageMap, StorageValue, ensure, debug::native as log,
};
use primitives::v1::{
	Balance, Hash, HrmpChannelId, Id as ParaId, InboundHrmpMessage, OutboundHrmpMessage,
	SessionIndex,
};
use sp_runtime::traits::{BlakeTwo256, Hash as HashT};
use sp_std::collections::{btree_set::BTreeSet, btree_map::BTreeMap};
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
#[cfg_attr(test, derive(Debug))]
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

const LOG_TARGET: &str = "runtime-parachains::hrmp";

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
					&& <paras::Module<T>>::is_valid_para(channel_id.recipient)
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

				let new_open_channel_req_cnt =
					<Self as Store>::HrmpOpenChannelRequestCount::get(&channel_id.sender)
						.saturating_sub(1);
				if new_open_channel_req_cnt != 0 {
					<Self as Store>::HrmpOpenChannelRequestCount::insert(
						&channel_id.sender,
						new_open_channel_req_cnt,
					);
				} else {
					<Self as Store>::HrmpOpenChannelRequestCount::remove(&channel_id.sender);
				}

				let new_accepted_channel_req_cnt =
					<Self as Store>::HrmpAcceptedChannelRequestCount::get(&channel_id.recipient)
						.saturating_sub(1);
				if new_accepted_channel_req_cnt != 0 {
					<Self as Store>::HrmpAcceptedChannelRequestCount::insert(
						&channel_id.recipient,
						new_accepted_channel_req_cnt,
					);
				} else {
					<Self as Store>::HrmpAcceptedChannelRequestCount::remove(&channel_id.recipient);
				}

				let _ = open_req_channels.swap_remove(idx);
				<Self as Store>::HrmpOpenChannelRequests::remove(&channel_id);
			} else {
				request.age += 1;
				if request.age == config.hrmp_open_request_ttl {
					// got stale

					<Self as Store>::HrmpOpenChannelRequestCount::mutate(&channel_id.sender, |v| {
						*v -= 1;
					});

					// TODO: return deposit https://github.com/paritytech/polkadot/issues/1907

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
		// TODO: return deposit https://github.com/paritytech/polkadot/issues/1907

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
				log::warn!(
					target: LOG_TARGET,
					"the HRMP watermark is not advanced relative to the last watermark  ({} > {})",
					new_hrmp_watermark,
					last_watermark,
				);
				return false;
			}
		}
		if new_hrmp_watermark > relay_chain_parent_number {
			log::warn!(
				target: LOG_TARGET,
				"the HRMP watermark is ahead the relay-parent ({} > {})",
				new_hrmp_watermark,
				relay_chain_parent_number,
			);
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
			if !digest
				.binary_search_by_key(&new_hrmp_watermark, |(block_no, _)| *block_no)
				.is_ok()
			{
				log::warn!(
					target: LOG_TARGET,
					"the HRMP watermark ({}) doesn't land on a block with messages received",
					new_hrmp_watermark,
				);
				return false;
			}
			true
		}
	}

	pub(crate) fn check_outbound_hrmp(
		config: &HostConfiguration<T::BlockNumber>,
		sender: ParaId,
		out_hrmp_msgs: &[OutboundHrmpMessage<ParaId>],
	) -> bool {
		if out_hrmp_msgs.len() as u32 > config.hrmp_max_message_num_per_candidate {
			log::warn!(
				target: LOG_TARGET,
				"more HRMP messages than permitted by config ({} > {})",
				out_hrmp_msgs.len(),
				config.hrmp_max_message_num_per_candidate,
			);
			return false;
		}

		let mut last_recipient = None::<ParaId>;

		for (idx, out_msg) in out_hrmp_msgs.iter().enumerate() {
			match last_recipient {
				// the messages must be sorted in ascending order and there must be no two messages sent
				// to the same recipient. Thus we can check that every recipient is strictly greater than
				// the previous one.
				Some(last_recipient) if out_msg.recipient <= last_recipient => {
					log::warn!(
						target: LOG_TARGET,
						"the HRMP messages are not sorted (at index {})",
						idx,
					);
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
				None => {
					log::warn!(
						target: LOG_TARGET,
						"the HRMP message at index {} is sent to a non existent channel {}->{}",
						idx,
						channel_id.sender,
						channel_id.recipient,
					);
					return false;
				}
			};

			if out_msg.data.len() as u32 > channel.max_message_size {
				log::warn!(
					target: LOG_TARGET,
					"the HRMP message at index {} exceeds the negotiated channel maximum message size ({} > {})",
					idx,
					out_msg.data.len(),
					channel.max_message_size,
				);
				return false;
			}

			let new_total_size = channel.total_size + out_msg.data.len() as u32;
			if new_total_size > channel.max_total_size {
				log::warn!(
					target: LOG_TARGET,
					"sending the HRMP message at index {} would exceed the neogitiated channel total size  ({} > {})",
					idx,
					new_total_size,
					channel.max_total_size,
				);
				return false;
			}

			let new_msg_count = channel.msg_count + 1;
			if new_msg_count > channel.max_capacity {
				log::warn!(
					target: LOG_TARGET,
					"sending the HRMP message at index {} would exceed the neogitiated channel capacity  ({} > {})",
					idx,
					new_msg_count,
					channel.max_capacity,
				);
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
			let mut leftover = Vec::with_capacity(digest.len());
			for (block_no, paras_sent_msg) in mem::replace(digest, Vec::new()) {
				if block_no <= new_hrmp_watermark {
					senders.extend(paras_sent_msg);
				} else {
					leftover.push((block_no, paras_sent_msg));
				}
			}
			*digest = leftover;
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
			let (mut pruned_cnt, mut pruned_size) = (0, 0);

			let contents = <Self as Store>::HrmpChannelContents::get(&channel_id);
			let mut leftover = Vec::with_capacity(contents.len());
			for msg in contents {
				if msg.sent_at <= new_hrmp_watermark {
					pruned_cnt += 1;
					pruned_size += msg.data.len();
				} else {
					leftover.push(msg);
				}
			}
			if !leftover.is_empty() {
				<Self as Store>::HrmpChannelContents::insert(&channel_id, leftover);
			} else {
				<Self as Store>::HrmpChannelContents::remove(&channel_id);
			}

			// update the channel metadata.
			<Self as Store>::HrmpChannels::mutate(&channel_id, |channel| {
				if let Some(ref mut channel) = channel {
					channel.msg_count -= pruned_cnt as u32;
					channel.total_size -= pruned_size as u32;
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

			// The digests are sorted in ascending by block number order. Assuming absence of
			// contextual execution, there are only two possible scenarios here:
			//
			// (a) It's the first time anybody sends a message to this recipient within this block.
			//     In this case, the digest vector would be empty or the block number of the latest
			//     entry  is smaller than the current.
			//
			// (b) Somebody has already sent a message within the current block. That means that
			//     the block number of the latest entry is equal to the current.
			//
			// Note that having the latest entry greater than the current block number is a logical
			// error.
			let mut recipient_digest =
				<Self as Store>::HrmpChannelDigests::get(&channel_id.recipient);
			if let Some(cur_block_digest) = recipient_digest
				.last_mut()
				.filter(|(block_no, _)| *block_no == now)
				.map(|(_, ref mut d)| d)
			{
				cur_block_digest.push(sender);
			} else {
				recipient_digest.push((now, vec![sender]));
			}
			<Self as Store>::HrmpChannelDigests::insert(&channel_id.recipient, recipient_digest);

			weight += T::DbWeight::get().reads_writes(2, 2);
		}

		weight
	}

	pub(super) fn init_open_channel(
		origin: ParaId,
		recipient: ParaId,
		proposed_max_capacity: u32,
		proposed_max_message_size: u32,
	) -> Result<(), DispatchError<T>> {
		ensure!(
			origin != recipient,
			DispatchError::<T>::OpenHrmpChannelToSelf
		);
		ensure!(
			<paras::Module<T>>::is_valid_para(recipient),
			DispatchError::<T>::OpenHrmpChannelInvalidRecipient,
		);

		let config = <configuration::Module<T>>::config();
		ensure!(
			proposed_max_capacity > 0,
			DispatchError::<T>::OpenHrmpChannelZeroCapacity,
		);
		ensure!(
			proposed_max_capacity <= config.hrmp_channel_max_capacity,
			DispatchError::<T>::OpenHrmpChannelCapacityExceedsLimit,
		);
		ensure!(
			proposed_max_message_size > 0,
			DispatchError::<T>::OpenHrmpChannelZeroMessageSize,
		);
		ensure!(
			proposed_max_message_size <= config.hrmp_channel_max_message_size,
			DispatchError::<T>::OpenHrmpChannelMessageSizeExceedsLimit,
		);

		let channel_id = HrmpChannelId {
			sender: origin,
			recipient,
		};
		ensure!(
			<Self as Store>::HrmpOpenChannelRequests::get(&channel_id).is_none(),
			DispatchError::<T>::OpenHrmpChannelAlreadyExists,
		);
		ensure!(
			<Self as Store>::HrmpChannels::get(&channel_id).is_none(),
			DispatchError::<T>::OpenHrmpChannelAlreadyRequested,
		);

		let egress_cnt =
			<Self as Store>::HrmpEgressChannelsIndex::decode_len(&origin).unwrap_or(0) as u32;
		let open_req_cnt = <Self as Store>::HrmpOpenChannelRequestCount::get(&origin);
		let channel_num_limit = if <paras::Module<T>>::is_parathread(origin) {
			config.hrmp_max_parathread_outbound_channels
		} else {
			config.hrmp_max_parachain_outbound_channels
		};
		ensure!(
			egress_cnt + open_req_cnt < channel_num_limit,
			DispatchError::<T>::OpenHrmpChannelLimitExceeded,
		);

		// TODO: Deposit https://github.com/paritytech/polkadot/issues/1907

		<Self as Store>::HrmpOpenChannelRequestCount::insert(&origin, open_req_cnt + 1);
		<Self as Store>::HrmpOpenChannelRequests::insert(
			&channel_id,
			HrmpOpenChannelRequest {
				confirmed: false,
				age: 0,
				sender_deposit: config.hrmp_sender_deposit,
				max_capacity: proposed_max_capacity,
				max_message_size: proposed_max_message_size,
				max_total_size: config.hrmp_channel_max_total_size,
			},
		);
		<Self as Store>::HrmpOpenChannelRequestsList::append(channel_id);

		let notification_bytes = {
			use xcm::v0::Xcm;
			use codec::Encode as _;

			Xcm::HrmpNewChannelOpenRequest {
				sender: u32::from(origin),
				max_capacity: proposed_max_capacity,
				max_message_size: proposed_max_message_size,
			}
			.encode()
		};
		if let Err(dmp::QueueDownwardMessageError::ExceedsMaxMessageSize) =
			Self::queue_downward_message(&config, recipient, notification_bytes)
		{
			// this should never happen unless the max downward message size is configured to an
			// jokingly small number.
			debug_assert!(false);
		}

		Ok(())
	}

	pub(super) fn accept_open_channel(
		origin: ParaId,
		sender: ParaId,
	) -> Result<(), DispatchError<T>> {
		let channel_id = HrmpChannelId {
			sender,
			recipient: origin,
		};
		let mut channel_req = <Self as Store>::HrmpOpenChannelRequests::get(&channel_id)
			.ok_or(DispatchError::<T>::AcceptHrmpChannelDoesntExist)?;
		ensure!(
			!channel_req.confirmed,
			DispatchError::<T>::AcceptHrmpChannelAlreadyConfirmed,
		);

		// check if by accepting this open channel request, this parachain would exceed the
		// number of inbound channels.
		let config = <configuration::Module<T>>::config();
		let channel_num_limit = if <paras::Module<T>>::is_parathread(origin) {
			config.hrmp_max_parathread_inbound_channels
		} else {
			config.hrmp_max_parachain_inbound_channels
		};
		let ingress_cnt =
			<Self as Store>::HrmpIngressChannelsIndex::decode_len(&origin).unwrap_or(0) as u32;
		let accepted_cnt = <Self as Store>::HrmpAcceptedChannelRequestCount::get(&origin);
		ensure!(
			ingress_cnt + accepted_cnt < channel_num_limit,
			DispatchError::<T>::AcceptHrmpChannelLimitExceeded,
		);

		// TODO: Deposit https://github.com/paritytech/polkadot/issues/1907

		// persist the updated open channel request and then increment the number of accepted
		// channels.
		channel_req.confirmed = true;
		<Self as Store>::HrmpOpenChannelRequests::insert(&channel_id, channel_req);
		<Self as Store>::HrmpAcceptedChannelRequestCount::insert(&origin, accepted_cnt + 1);

		let notification_bytes = {
			use xcm::v0::Xcm;
			use codec::Encode as _;

			Xcm::HrmpChannelAccepted {
				recipient: u32::from(origin),
			}
			.encode()
		};
		if let Err(dmp::QueueDownwardMessageError::ExceedsMaxMessageSize) =
			Self::queue_downward_message(&config, sender, notification_bytes)
		{
			// this should never happen unless the max downward message size is configured to an
			// jokingly small number.
			debug_assert!(false);
		}

		Ok(())
	}

	pub(super) fn close_channel(
		origin: ParaId,
		channel_id: HrmpChannelId,
	) -> Result<(), DispatchError<T>> {
		// check if the origin is allowed to close the channel.
		ensure!(
			origin == channel_id.sender || origin == channel_id.recipient,
			DispatchError::<T>::CloseHrmpChannelUnauthorized,
		);

		// check if the channel requested to close does exist.
		ensure!(
			<Self as Store>::HrmpChannels::get(&channel_id).is_some(),
			DispatchError::<T>::CloseHrmpChannelDoesntExist,
		);

		// check that there is no outstanding close request for this channel
		ensure!(
			<Self as Store>::HrmpCloseChannelRequests::get(&channel_id).is_none(),
			DispatchError::<T>::CloseHrmpChannelAlreadyUnderway,
		);

		<Self as Store>::HrmpCloseChannelRequests::insert(&channel_id, ());
		<Self as Store>::HrmpCloseChannelRequestsList::append(channel_id.clone());

		let config = <configuration::Module<T>>::config();
		let notification_bytes = {
			use xcm::v0::Xcm;
			use codec::Encode as _;

			Xcm::HrmpChannelClosing {
				initiator: u32::from(origin),
				sender: u32::from(channel_id.sender),
				recipient: u32::from(channel_id.recipient),
			}
			.encode()
		};
		let opposite_party = if origin == channel_id.sender {
			channel_id.recipient
		} else {
			channel_id.sender
		};
		if let Err(dmp::QueueDownwardMessageError::ExceedsMaxMessageSize) =
			Self::queue_downward_message(&config, opposite_party, notification_bytes)
		{
			// this should never happen unless the max downward message size is configured to an
			// jokingly small number.
			debug_assert!(false);
		}

		Ok(())
	}

	/// Returns the list of MQC heads for the inbound channels of the given recipient para paired
	/// with the sender para ids. This vector is sorted ascending by the para id and doesn't contain
	/// multiple entries with the same sender.
	pub(crate) fn hrmp_mqc_heads(recipient: ParaId) -> Vec<(ParaId, Hash)> {
		let sender_set = <Self as Store>::HrmpIngressChannelsIndex::get(&recipient);

		// The ingress channels vector is sorted, thus `mqc_heads` is sorted as well.
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

	/// Returns contents of all channels addressed to the given recipient. Channels that have no
	/// messages in them are also included.
	pub(crate) fn inbound_hrmp_channels_contents(
		recipient: ParaId,
	) -> BTreeMap<ParaId, Vec<InboundHrmpMessage<T::BlockNumber>>> {
		let sender_set = <Self as Store>::HrmpIngressChannelsIndex::get(&recipient);

		let mut inbound_hrmp_channels_contents = BTreeMap::new();
		for sender in sender_set {
			let channel_contents =
				<Self as Store>::HrmpChannelContents::get(&HrmpChannelId { sender, recipient });
			inbound_hrmp_channels_contents.insert(sender, channel_contents);
		}

		inbound_hrmp_channels_contents
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::router::tests::default_genesis_config;
	use crate::mock::{Configuration, System, Paras, Router, new_test_ext};
	use primitives::v1::BlockNumber;
	use std::collections::{BTreeMap, HashSet};

	pub(crate) fn run_to_block(to: BlockNumber, new_session: Option<Vec<BlockNumber>>) {
		use frame_support::traits::{OnFinalize as _, OnInitialize as _};

		while System::block_number() < to {
			let b = System::block_number();

			// NOTE: this is in reverse initialization order.
			Router::initializer_finalize();
			Paras::initializer_finalize();

			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			if new_session.as_ref().map_or(false, |v| v.contains(&(b + 1))) {
				// NOTE: this is in initialization order.
				Paras::initializer_on_new_session(&Default::default());
				Router::initializer_on_new_session(&Default::default());
			}

			// NOTE: this is in initialization order.
			Paras::initializer_initialize(b + 1);
			Router::initializer_initialize(b + 1);
		}
	}

	struct GenesisConfigBuilder {
		hrmp_channel_max_capacity: u32,
		hrmp_channel_max_message_size: u32,
		hrmp_max_parathread_outbound_channels: u32,
		hrmp_max_parachain_outbound_channels: u32,
		hrmp_max_parathread_inbound_channels: u32,
		hrmp_max_parachain_inbound_channels: u32,
		hrmp_max_message_num_per_candidate: u32,
		hrmp_channel_max_total_size: u32,
	}

	impl Default for GenesisConfigBuilder {
		fn default() -> Self {
			Self {
				hrmp_channel_max_capacity: 2,
				hrmp_channel_max_message_size: 8,
				hrmp_max_parathread_outbound_channels: 1,
				hrmp_max_parachain_outbound_channels: 2,
				hrmp_max_parathread_inbound_channels: 1,
				hrmp_max_parachain_inbound_channels: 2,
				hrmp_max_message_num_per_candidate: 2,
				hrmp_channel_max_total_size: 16,
			}
		}
	}

	impl GenesisConfigBuilder {
		fn build(self) -> crate::mock::GenesisConfig {
			let mut genesis = default_genesis_config();
			let config = &mut genesis.configuration.config;
			config.hrmp_channel_max_capacity = self.hrmp_channel_max_capacity;
			config.hrmp_channel_max_message_size = self.hrmp_channel_max_message_size;
			config.hrmp_max_parathread_outbound_channels =
				self.hrmp_max_parathread_outbound_channels;
			config.hrmp_max_parachain_outbound_channels = self.hrmp_max_parachain_outbound_channels;
			config.hrmp_max_parathread_inbound_channels = self.hrmp_max_parathread_inbound_channels;
			config.hrmp_max_parachain_inbound_channels = self.hrmp_max_parachain_inbound_channels;
			config.hrmp_max_message_num_per_candidate = self.hrmp_max_message_num_per_candidate;
			config.hrmp_channel_max_total_size = self.hrmp_channel_max_total_size;
			genesis
		}
	}

	fn register_parachain(id: ParaId) {
		Paras::schedule_para_initialize(
			id,
			crate::paras::ParaGenesisArgs {
				parachain: true,
				genesis_head: vec![1].into(),
				validation_code: vec![1].into(),
			},
		);
	}

	fn deregister_parachain(id: ParaId) {
		Paras::schedule_para_cleanup(id);
	}

	fn channel_exists(sender: ParaId, recipient: ParaId) -> bool {
		<Router as Store>::HrmpChannels::get(&HrmpChannelId { sender, recipient }).is_some()
	}

	fn assert_storage_consistency_exhaustive() {
		use frame_support::IterableStorageMap;

		assert_eq!(
			<Router as Store>::HrmpOpenChannelRequests::iter()
				.map(|(k, _)| k)
				.collect::<HashSet<_>>(),
			<Router as Store>::HrmpOpenChannelRequestsList::get()
				.into_iter()
				.collect::<HashSet<_>>(),
		);

		// verify that the set of keys in `HrmpOpenChannelRequestCount` corresponds to the set
		// of _senders_ in `HrmpOpenChannelRequests`.
		//
		// having ensured that, we can go ahead and go over all counts and verify that they match.
		assert_eq!(
			<Router as Store>::HrmpOpenChannelRequestCount::iter()
				.map(|(k, _)| k)
				.collect::<HashSet<_>>(),
			<Router as Store>::HrmpOpenChannelRequests::iter()
				.map(|(k, _)| k.sender)
				.collect::<HashSet<_>>(),
		);
		for (open_channel_initiator, expected_num) in
			<Router as Store>::HrmpOpenChannelRequestCount::iter()
		{
			let actual_num = <Router as Store>::HrmpOpenChannelRequests::iter()
				.filter(|(ch, _)| ch.sender == open_channel_initiator)
				.count() as u32;
			assert_eq!(expected_num, actual_num);
		}

		// The same as above, but for accepted channel request count. Note that we are interested
		// only in confirmed open requests.
		assert_eq!(
			<Router as Store>::HrmpAcceptedChannelRequestCount::iter()
				.map(|(k, _)| k)
				.collect::<HashSet<_>>(),
			<Router as Store>::HrmpOpenChannelRequests::iter()
				.filter(|(_, v)| v.confirmed)
				.map(|(k, _)| k.recipient)
				.collect::<HashSet<_>>(),
		);
		for (channel_recipient, expected_num) in
			<Router as Store>::HrmpAcceptedChannelRequestCount::iter()
		{
			let actual_num = <Router as Store>::HrmpOpenChannelRequests::iter()
				.filter(|(ch, v)| ch.recipient == channel_recipient && v.confirmed)
				.count() as u32;
			assert_eq!(expected_num, actual_num);
		}

		assert_eq!(
			<Router as Store>::HrmpCloseChannelRequests::iter()
				.map(|(k, _)| k)
				.collect::<HashSet<_>>(),
			<Router as Store>::HrmpCloseChannelRequestsList::get()
				.into_iter()
				.collect::<HashSet<_>>(),
		);

		// A HRMP watermark can be None for an onboarded parachain. However, an offboarded parachain
		// cannot have an HRMP watermark: it should've been cleanup.
		assert_contains_only_onboarded(
			<Router as Store>::HrmpWatermarks::iter().map(|(k, _)| k),
			"HRMP watermarks should contain only onboarded paras",
		);

		// An entry in `HrmpChannels` indicates that the channel is open. Only open channels can
		// have contents.
		for (non_empty_channel, contents) in <Router as Store>::HrmpChannelContents::iter() {
			assert!(<Router as Store>::HrmpChannels::contains_key(
				&non_empty_channel
			));

			// pedantic check: there should be no empty vectors in storage, those should be modeled
			// by a removed kv pair.
			assert!(!contents.is_empty());
		}

		// Senders and recipients must be onboarded. Otherwise, all channels associated with them
		// are removed.
		assert_contains_only_onboarded(
			<Router as Store>::HrmpChannels::iter().flat_map(|(k, _)| vec![k.sender, k.recipient]),
			"senders and recipients in all channels should be onboarded",
		);

		// Check the docs for `HrmpIngressChannelsIndex` and `HrmpEgressChannelsIndex` in decl
		// storage to get an index what are the channel mappings indexes.
		//
		// Here, from indexes.
		//
		// ingress         egress
		//
		// a -> [x, y]     x -> [a, b]
		// b -> [x, z]     y -> [a]
		//                 z -> [b]
		//
		// we derive a list of channels they represent.
		//
		//   (a, x)         (a, x)
		//   (a, y)         (a, y)
		//   (b, x)         (b, x)
		//   (b, z)         (b, z)
		//
		// and then that we compare that to the channel list in the `HrmpChannels`.
		let channel_set_derived_from_ingress = <Router as Store>::HrmpIngressChannelsIndex::iter()
			.flat_map(|(p, v)| v.into_iter().map(|i| (i, p)).collect::<Vec<_>>())
			.collect::<HashSet<_>>();
		let channel_set_derived_from_egress = <Router as Store>::HrmpEgressChannelsIndex::iter()
			.flat_map(|(p, v)| v.into_iter().map(|e| (p, e)).collect::<Vec<_>>())
			.collect::<HashSet<_>>();
		let channel_set_ground_truth = <Router as Store>::HrmpChannels::iter()
			.map(|(k, _)| (k.sender, k.recipient))
			.collect::<HashSet<_>>();
		assert_eq!(
			channel_set_derived_from_ingress,
			channel_set_derived_from_egress
		);
		assert_eq!(channel_set_derived_from_egress, channel_set_ground_truth);

		<Router as Store>::HrmpIngressChannelsIndex::iter()
			.map(|(_, v)| v)
			.for_each(|v| assert_is_sorted(&v, "HrmpIngressChannelsIndex"));
		<Router as Store>::HrmpEgressChannelsIndex::iter()
			.map(|(_, v)| v)
			.for_each(|v| assert_is_sorted(&v, "HrmpIngressChannelsIndex"));

		assert_contains_only_onboarded(
			<Router as Store>::HrmpChannelDigests::iter().map(|(k, _)| k),
			"HRMP channel digests should contain only onboarded paras",
		);
		for (_digest_for_para, digest) in <Router as Store>::HrmpChannelDigests::iter() {
			// Assert that items are in **strictly** ascending order. The strictness also implies
			// there are no duplicates.
			assert!(digest.windows(2).all(|xs| xs[0].0 < xs[1].0));

			for (_, mut senders) in digest {
				assert!(!senders.is_empty());

				// check for duplicates. For that we sort the vector, then perform deduplication.
				// if the vector stayed the same, there are no duplicates.
				senders.sort();
				let orig_senders = senders.clone();
				senders.dedup();
				assert_eq!(
					orig_senders, senders,
					"duplicates removed implies existence of duplicates"
				);
			}
		}

		fn assert_contains_only_onboarded(iter: impl Iterator<Item = ParaId>, cause: &str) {
			for para in iter {
				assert!(
					Paras::is_valid_para(para),
					"{}: {} para is offboarded",
					cause,
					para
				);
			}
		}
	}

	fn assert_is_sorted<T: Ord>(slice: &[T], id: &str) {
		assert!(
			slice.windows(2).all(|xs| xs[0] <= xs[1]),
			"{} supposed to be sorted",
			id
		);
	}

	#[test]
	fn empty_state_consistent_state() {
		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn open_channel_works() {
		let para_a = 1.into();
		let para_b = 3.into();

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			// We need both A & B to be registered and alive parachains.
			register_parachain(para_a);
			register_parachain(para_b);

			run_to_block(5, Some(vec![5]));
			Router::init_open_channel(para_a, para_b, 2, 8).unwrap();
			assert_storage_consistency_exhaustive();

			Router::accept_open_channel(para_b, para_a).unwrap();
			assert_storage_consistency_exhaustive();

			// Advance to a block 6, but without session change. That means that the channel has
			// not been created yet.
			run_to_block(6, None);
			assert!(!channel_exists(para_a, para_b));
			assert_storage_consistency_exhaustive();

			// Now let the session change happen and thus open the channel.
			run_to_block(8, Some(vec![8]));
			assert!(channel_exists(para_a, para_b));
		});
	}

	#[test]
	fn close_channel_works() {
		let para_a = 5.into();
		let para_b = 2.into();

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			register_parachain(para_a);
			register_parachain(para_b);

			run_to_block(5, Some(vec![5]));
			Router::init_open_channel(para_a, para_b, 2, 8).unwrap();
			Router::accept_open_channel(para_b, para_a).unwrap();

			run_to_block(6, Some(vec![6]));
			assert!(channel_exists(para_a, para_b));

			// Close the channel. The effect is not immediate, but rather deferred to the next
			// session change.
			Router::close_channel(
				para_b,
				HrmpChannelId {
					sender: para_a,
					recipient: para_b,
				},
			)
			.unwrap();
			assert!(channel_exists(para_a, para_b));
			assert_storage_consistency_exhaustive();

			// After the session change the channel should be closed.
			run_to_block(8, Some(vec![8]));
			assert!(!channel_exists(para_a, para_b));
			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn send_recv_messages() {
		let para_a = 32.into();
		let para_b = 64.into();

		let mut genesis = GenesisConfigBuilder::default();
		genesis.hrmp_channel_max_message_size = 20;
		genesis.hrmp_channel_max_total_size = 20;
		new_test_ext(genesis.build()).execute_with(|| {
			register_parachain(para_a);
			register_parachain(para_b);

			run_to_block(5, Some(vec![5]));
			Router::init_open_channel(para_a, para_b, 2, 20).unwrap();
			Router::accept_open_channel(para_b, para_a).unwrap();

			// On Block 6:
			// A sends a message to B
			run_to_block(6, Some(vec![6]));
			assert!(channel_exists(para_a, para_b));
			let msgs = vec![OutboundHrmpMessage {
				recipient: para_b,
				data: b"this is an emergency".to_vec(),
			}];
			let config = Configuration::config();
			assert!(Router::check_outbound_hrmp(&config, para_a, &msgs));
			let _ = Router::queue_outbound_hrmp(para_a, msgs);
			assert_storage_consistency_exhaustive();

			// On Block 7:
			// B receives the message sent by A. B sets the watermark to 6.
			run_to_block(7, None);
			assert!(Router::check_hrmp_watermark(para_b, 7, 6));
			let _ = Router::prune_hrmp(para_b, 6);
			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn accept_incoming_request_and_offboard() {
		let para_a = 32.into();
		let para_b = 64.into();

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			register_parachain(para_a);
			register_parachain(para_b);

			run_to_block(5, Some(vec![5]));
			Router::init_open_channel(para_a, para_b, 2, 8).unwrap();
			Router::accept_open_channel(para_b, para_a).unwrap();
			deregister_parachain(para_a);

			// On Block 6: session change. The channel should not be created.
			run_to_block(6, Some(vec![6]));
			assert!(!Paras::is_valid_para(para_a));
			assert!(!channel_exists(para_a, para_b));
			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn check_sent_messages() {
		let para_a = 32.into();
		let para_b = 64.into();
		let para_c = 97.into();

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			register_parachain(para_a);
			register_parachain(para_b);
			register_parachain(para_c);

			run_to_block(5, Some(vec![5]));

			// Open two channels to the same receiver, b:
			// a -> b, c -> b
			Router::init_open_channel(para_a, para_b, 2, 8).unwrap();
			Router::accept_open_channel(para_b, para_a).unwrap();
			Router::init_open_channel(para_c, para_b, 2, 8).unwrap();
			Router::accept_open_channel(para_b, para_c).unwrap();

			// On Block 6: session change.
			run_to_block(6, Some(vec![6]));
			assert!(Paras::is_valid_para(para_a));

			let msgs = vec![OutboundHrmpMessage {
				recipient: para_b,
				data: b"knock".to_vec(),
			}];
			let config = Configuration::config();
			assert!(Router::check_outbound_hrmp(&config, para_a, &msgs));
			let _ = Router::queue_outbound_hrmp(para_a, msgs.clone());

			// Verify that the sent messages are there and that also the empty channels are present.
			let mqc_heads = Router::hrmp_mqc_heads(para_b);
			let contents = Router::inbound_hrmp_channels_contents(para_b);
			assert_eq!(
				contents,
				vec![
					(
						para_a,
						vec![InboundHrmpMessage {
							sent_at: 6,
							data: b"knock".to_vec(),
						}]
					),
					(para_c, vec![])
				]
				.into_iter()
				.collect::<BTreeMap::<_, _>>(),
			);
			assert_eq!(
				mqc_heads,
				vec![
					(
						para_a,
						hex_literal::hex!(
							"3bba6404e59c91f51deb2ae78f1273ebe75896850713e13f8c0eba4b0996c483"
						)
						.into()
					),
					(para_c, Default::default())
				],
			);

			assert_storage_consistency_exhaustive();
		});
	}
}
