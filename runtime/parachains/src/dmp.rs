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

use crate::{
	configuration::{self, HostConfiguration},
	initializer,
};

use frame_support::pallet_prelude::*;
use primitives::v2::{DownwardMessage, Hash, Id as ParaId, InboundDownwardMessage, QueueMessageId};
use sp_runtime::traits::{BlakeTwo256, Hash as HashT, SaturatedConversion};
use sp_std::{fmt, prelude::*};
use xcm::latest::SendError;

pub use pallet::*;

#[cfg(test)]
mod tests;

pub mod migration;

/// The key for a group of downward messages.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct QueuePageIdx(ParaId, u64);

/// An error sending a downward message.
#[derive(Debug)]
pub enum QueueDownwardMessageError {
	/// The message being sent exceeds the configured max message size.
	ExceedsMaxMessageSize,
}

impl From<QueueDownwardMessageError> for SendError {
	fn from(err: QueueDownwardMessageError) -> Self {
		match err {
			QueueDownwardMessageError::ExceedsMaxMessageSize => SendError::ExceedsMaxMessageSize,
		}
	}
}

/// An error returned by [`check_processed_downward_messages`] that indicates an acceptance check
/// didn't pass.
pub enum ProcessedDownwardMessagesAcceptanceErr {
	/// If there are pending messages then `processed_downward_messages` should be at least 1,
	AdvancementRule,
	/// `processed_downward_messages` should not be greater than the number of pending messages.
	Underflow { processed_downward_messages: u32, dmq_length: u32 },
}

impl fmt::Debug for ProcessedDownwardMessagesAcceptanceErr {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		use ProcessedDownwardMessagesAcceptanceErr::*;
		match *self {
			AdvancementRule =>
				write!(fmt, "DMQ is not empty, but processed_downward_messages is 0",),
			Underflow { processed_downward_messages, dmq_length } => write!(
				fmt,
				"processed_downward_messages = {}, but dmq_length is only {}",
				processed_downward_messages, dmq_length,
			),
		}
	}
}

/// To readjust the memory footprint when sending or receiving messages we will split
/// the queue in pages of `QUEUE_PAGE_CAPACITY` capacity. Tuning this constant allows
/// to control how we trade off the overhead per stored message vs memory footprint of individual
/// messages read. The pages are part of ring buffer per para and we keep track of the head and tail page index.
///
/// TODO(maybe) - make these configuration parameters?
///
/// Defines the queue page capacity.
pub const QUEUE_PAGE_CAPACITY: u32 = 32;
/// Defines the maximum amount of pages returned by `dmq_contents`.
pub const MAX_PAGES_PER_QUERY: u32 = 2;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::storage_version(migration::STORAGE_VERSION)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config {}

	#[pallet::storage]
	#[pallet::getter(fn dmp_queue_head)]
	pub(super) type DownwardMessageQueueHead<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, u64, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn dmp_queue_tail)]
	pub(super) type DownwardMessageQueueTail<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, u64, ValueQuery>;

	/// The current downward message index bounds.
	#[pallet::storage]
	#[pallet::getter(fn dmp_message_idx)]
	pub(super) type DownwardMessageIdx<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, (u64, u64), ValueQuery>;

	/// The downward messages addressed for a certain para.
	#[pallet::storage]
	pub(crate) type DownwardMessageQueuePages<T: Config> = StorageMap<
		_,
		Twox64Concat,
		QueuePageIdx,
		Vec<InboundDownwardMessage<T::BlockNumber>>,
		ValueQuery,
	>;

	/// A mapping that stores the downward message queue MQC head for each para.
	///
	/// Each link in this chain has a form:
	/// `(prev_head, B, H(M))`, where
	/// - `prev_head`: is the previous head hash or zero if none.
	/// - `B`: is the relay-chain block number in which a message was appended.
	/// - `H(M)`: is the hash of the message being appended.
	#[pallet::storage]
	pub(crate) type DownwardMessageQueueHeads<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, Hash, ValueQuery>;

	/// A mapping between a message and MQC head.
	#[pallet::storage]
	pub(crate) type DownwardMessageQueueHeadsById<T: Config> =
		StorageMap<_, Twox64Concat, QueueMessageId, Hash, ValueQuery>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

/// Routines and getters related to downward message passing.
impl<T: Config> Pallet<T> {
	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(
		_notification: &initializer::SessionChangeNotification<T::BlockNumber>,
		outgoing_paras: &[ParaId],
	) {
		Self::perform_outgoing_para_cleanup(outgoing_paras);
	}

	/// Iterate over all paras that were noted for offboarding and remove all the data
	/// associated with them.
	fn perform_outgoing_para_cleanup(outgoing: &[ParaId]) {
		for outgoing_para in outgoing {
			Self::clean_dmp_after_outgoing(outgoing_para);
		}
	}

	pub(crate) fn update_head(para: &ParaId, new_head: u64) -> Weight {
		<Self as Store>::DownwardMessageQueueHead::mutate(para, |head_pointer| {
			*head_pointer = new_head;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	pub(crate) fn update_tail(para: &ParaId, new_tail: u64) -> Weight {
		<Self as Store>::DownwardMessageQueueTail::mutate(para, |tail_pointer| {
			*tail_pointer = new_tail;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	fn update_first_message_idx(para: &ParaId, new_message_id: u64) -> Weight {
		<Self as Store>::DownwardMessageIdx::mutate(para, |message_idx| {
			message_idx.0 = new_message_id;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	fn update_last_message_idx(para: &ParaId, new_message_id: u64) -> Weight {
		<Self as Store>::DownwardMessageIdx::mutate(para, |message_idx| {
			message_idx.1 = new_message_id;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	/// Remove all relevant storage items for an outgoing parachain.
	fn clean_dmp_after_outgoing(outgoing_para: &ParaId) {
		for index in Self::dmp_queue_head(outgoing_para)..=Self::dmp_queue_tail(outgoing_para) {
			<Self as Store>::DownwardMessageQueuePages::remove(QueuePageIdx(*outgoing_para, index));
		}

		<Self as Store>::DownwardMessageQueueHeads::remove(outgoing_para);
	}

	/// Enqueue a downward message to a specific recipient para.
	///
	/// When encoded, the message should not exceed the `config.max_downward_message_size`.
	/// Otherwise, the message won't be sent and `Err` will be returned.
	///
	/// It is possible to send a downward message to a non-existent para. That, however, would lead
	/// to a dangling storage. If the caller cannot statically prove that the recipient exists
	/// then the caller should perform a runtime check.
	pub fn queue_downward_message(
		config: &HostConfiguration<T::BlockNumber>,
		para: ParaId,
		msg: DownwardMessage,
	) -> Result<Weight, QueueDownwardMessageError> {
		// Check if message is oversized.
		let serialized_len = msg.len() as u32;
		if serialized_len > config.max_downward_message_size {
			return Err(QueueDownwardMessageError::ExceedsMaxMessageSize)
		}

		let mut weight = 0u64;
		let head = Self::dmp_queue_head(para);
		let mut tail = Self::dmp_queue_tail(para);
		// Get first/last message indexes.
		let message_idx = Self::dmp_message_idx(para);
		let current_message_idx = message_idx.1.wrapping_add(1);

		// Ring buffer is empty.
		if head == tail {
			// Tail always points to the next unused page.
			tail = tail.wrapping_add(1);
			weight = weight.saturating_add(Self::update_tail(&para, tail).into());
		}

		let tail_page_len = <Self as Store>::DownwardMessageQueuePages::decode_len(QueuePageIdx(
			para,
			tail.wrapping_sub(1),
		))
		.unwrap_or(0)
		.saturated_into::<u32>();

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 0));

		// Check if we need a new page.
		if tail_page_len >= QUEUE_PAGE_CAPACITY {
			// In practice this is always bounded economically.
			if tail.wrapping_add(1) == head {
				unimplemented!("The end of the world is upon us");
			}

			// Advance tail.
			tail = tail.wrapping_add(1);
		}

		let inbound =
			InboundDownwardMessage { msg, sent_at: <frame_system::Pallet<T>>::block_number() };

		// Obtain the new link in the MQC and update the head.
		<Self as Store>::DownwardMessageQueueHeads::mutate(para, |head| {
			let new_head =
				BlakeTwo256::hash_of(&(*head, inbound.sent_at, T::Hashing::hash_of(&inbound.msg)));
			*head = new_head;

			// Update the head for the current message.
			<Self as Store>::DownwardMessageQueueHeadsById::mutate(
				QueueMessageId(para, current_message_idx),
				|head| *head = new_head,
			);
		});

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(2, 2));

		// Insert message in the tail queue page.
		<Self as Store>::DownwardMessageQueuePages::mutate(
			QueuePageIdx(para, tail.wrapping_sub(1)),
			|v| {
				v.push(inbound);
			},
		);

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 1));
		weight = weight.saturating_add(Self::update_tail(&para, tail));
		weight = weight.saturating_add(Self::update_last_message_idx(&para, current_message_idx));

		Ok(weight)
	}

	/// Checks if the number of processed downward messages is valid.
	pub(crate) fn check_processed_downward_messages(
		para: ParaId,
		processed_downward_messages: u32,
	) -> Result<(), ProcessedDownwardMessagesAcceptanceErr> {
		let dmq_length = Self::dmq_length(para);

		if dmq_length > 0 && processed_downward_messages == 0 {
			return Err(ProcessedDownwardMessagesAcceptanceErr::AdvancementRule)
		}
		if dmq_length < processed_downward_messages {
			return Err(ProcessedDownwardMessagesAcceptanceErr::Underflow {
				processed_downward_messages,
				dmq_length,
			})
		}

		Ok(())
	}

	fn mqc_head_key_range(para: ParaId, start: u64, count: u64) -> Vec<QueueMessageId> {
		let mut keys = Vec::new();
		let mut idx = start;
		while idx != start.wrapping_add(count) {
			keys.push(QueueMessageId(para, idx));
			idx = idx.wrapping_add(1);
		}

		keys
	}

	/// Prunes the specified number of messages from the downward message queue of the given para.
	pub(crate) fn prune_dmq(para: ParaId, processed_downward_messages: u32) -> Weight {
		let mut head = Self::dmp_queue_head(para);
		let tail = Self::dmp_queue_tail(para);
		let mut messages_to_prune = processed_downward_messages as u64;
		let mut first_message_idx = Self::dmp_message_idx(para).0;
		let mut total_weight = T::DbWeight::get().reads_writes(3, 0);

		// TODO: also remove `DownwardMessageQueueHeadsById` entries.
		let mut queue_keys_to_remove = Vec::new();
		let mut mqc_keys_to_remove = Vec::new();

		// Determine the keys to be removed.
		while messages_to_prune > 0 && head != tail {
			let key = QueuePageIdx(para, head);
			<Self as Store>::DownwardMessageQueuePages::mutate(key.clone(), |q| {
				let messages_in_page = q.len() as u64;

				if messages_to_prune >= messages_in_page {
					messages_to_prune = messages_to_prune.saturating_sub(messages_in_page);

					// Add mutate weight. Removal happens later.
					total_weight += T::DbWeight::get().reads_writes(1, 1);
					queue_keys_to_remove.push(key);

					mqc_keys_to_remove.extend(Self::mqc_head_key_range(
						para,
						first_message_idx,
						messages_in_page,
					));

					// Advance first message index.
					first_message_idx = first_message_idx.wrapping_add(messages_in_page);

					// Advance head.
					head = head.wrapping_add(1);
				} else {
					mqc_keys_to_remove.extend(Self::mqc_head_key_range(
						para,
						first_message_idx,
						messages_to_prune,
					));

					first_message_idx = first_message_idx.wrapping_add(messages_to_prune);
					*q = q.split_off(messages_to_prune as usize);

					// Only account for the update. This key will not be deleted.
					total_weight += T::DbWeight::get().reads_writes(1, 1);

					// Break loop.
					messages_to_prune = 0;
				}
			});
		}

		// Actually do cleanup.
		for key in queue_keys_to_remove {
			<Self as Store>::DownwardMessageQueuePages::remove(key);
		}

		for key in mqc_keys_to_remove {
			<Self as Store>::DownwardMessageQueueHeadsById::remove(key);
		}

		// Update indexes and mqc head.
		Self::update_first_message_idx(&para, first_message_idx) +
			Self::update_head(&para, head) +
			total_weight
	}

	/// Returns the Head of Message Queue Chain for the given para or `None` if there is none
	/// associated with it.
	#[cfg(test)]
	fn dmq_mqc_head(para: ParaId) -> Hash {
		<Self as Store>::DownwardMessageQueueHeads::get(&para)
	}

	#[cfg(test)]
	fn dmq_mqc_head_for_message(para: ParaId, message_index: u64) -> Hash {
		<Self as Store>::DownwardMessageQueueHeadsById::get(&QueueMessageId(para, message_index))
	}

	/// Returns the number of pending downward messages addressed to the given para.
	///
	/// Returns 0 if the para doesn't have an associated downward message queue.
	pub(crate) fn dmq_length(para: ParaId) -> u32 {
		let mut head = Self::dmp_queue_head(para);
		let tail = Self::dmp_queue_tail(para);
		let mut length = 0;

		while head != tail {
			length +=
				<Self as Store>::DownwardMessageQueuePages::decode_len(QueuePageIdx(para, head))
					.unwrap_or(0)
					.saturated_into::<u32>();
			head = head.wrapping_add(1);
		}

		length
	}

	/// Deprecated API. Please use `dmq_contents_bounded`.
	pub(crate) fn dmq_contents(recipient: ParaId) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		Self::dmq_contents_bounded(recipient, 0, MAX_PAGES_PER_QUERY)
	}

	/// Returns `count` pages starting from the page index specified by `start`.
	/// The result will be an empty vector if the para doesn't exist or it's queue is empty.
	///
	/// The result preserves the original message ordering - first element of vec is oldest message, while last is most
	/// recent.
	pub(crate) fn dmq_contents_bounded(
		recipient: ParaId,
		start: u32,
		mut count: u32,
	) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		let mut head = Self::dmp_queue_head(recipient);
		let tail = Self::dmp_queue_tail(recipient);
		let mut result = Vec::new();
		let mut pages_to_skip = start;

		// Loop until we reach the tail, or we've gathered at least `count` pages.
		while head != tail && count > 0 {
			if pages_to_skip == 0 {
				result.extend(<Self as Store>::DownwardMessageQueuePages::get(QueuePageIdx(
					recipient, head,
				)));
				count -= 1;
			} else {
				pages_to_skip -= 1;
			}

			// Advance to next page.
			head = head.wrapping_add(1);
		}

		result
	}
}
