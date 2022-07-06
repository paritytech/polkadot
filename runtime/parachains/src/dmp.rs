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
use sp_std::{fmt, num::Wrapping, prelude::*};
use xcm::latest::SendError;

pub use pallet::*;

#[cfg(test)]
mod tests;

/// The key for a group of downward messages.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct QueueFragmentIdx(ParaId, u64);

/// An error sending a downward message.
#[cfg_attr(test, derive(Debug))]
pub enum QueueDownwardMessageError {
	/// The message being sent exceeds the configured max message size.
	ExceedsMaxMessageSize,
	/// The message cannot be sent because the destination parachain message queue is full.
	ExceedsMaxPendingMessageCount,
}

impl From<QueueDownwardMessageError> for SendError {
	fn from(err: QueueDownwardMessageError) -> Self {
		match err {
			QueueDownwardMessageError::ExceedsMaxMessageSize => SendError::ExceedsMaxMessageSize,
			QueueDownwardMessageError::ExceedsMaxPendingMessageCount =>
				SendError::ExceedsMaxPendingMessageCount,
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

/// To reduce the runtime memory footprint when sending or receiving messages we will split
/// the queue in fragments of `QUEUE_FRAGMENT_CAPACITY` capacity. Tuning this constant allows
/// to control how we trade off the overhead per stored message vs memory footprint of individual
/// message read/writes from/to storage. The fragments are part of circular buffer per para and
/// we keep track of the head and tail fragments.
///
/// TODO(maybe) - make these configuration parameters?
///
/// Defines the queue fragment capacity.
pub const QUEUE_FRAGMENT_CAPACITY: u32 = 1;
/// Defines the maximum amount of messages returned by `dmq_contents`/`dmq_contents_bounded`.
pub const MAX_MESSAGES_PER_QUERY: u32 = 2048;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
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
	pub(crate) type DownwardMessageQueueFragments<T: Config> = StorageMap<
		_,
		Twox64Concat,
		QueueFragmentIdx,
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

	fn update_head(para: &ParaId, new_head: Wrapping<u64>) -> Weight {
		<Self as Store>::DownwardMessageQueueHead::mutate(para, |head_pointer| {
			*head_pointer = new_head.0;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	fn update_tail(para: &ParaId, new_tail: Wrapping<u64>) -> Weight {
		<Self as Store>::DownwardMessageQueueTail::mutate(para, |tail_pointer| {
			*tail_pointer = new_tail.0;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	fn update_first_message_idx(para: &ParaId, new_message_id: Wrapping<u64>) -> Weight {
		<Self as Store>::DownwardMessageIdx::mutate(para, |message_idx| {
			message_idx.0 = new_message_id.0;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	fn update_last_message_idx(para: &ParaId, new_message_id: Wrapping<u64>) -> Weight {
		<Self as Store>::DownwardMessageIdx::mutate(para, |message_idx| {
			message_idx.1 = new_message_id.0;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	/// Remove all relevant storage items for an outgoing parachain.
	fn clean_dmp_after_outgoing(outgoing_para: &ParaId) {
		for index in Self::dmp_queue_head(outgoing_para)..=Self::dmp_queue_tail(outgoing_para) {
			<Self as Store>::DownwardMessageQueueFragments::remove(QueueFragmentIdx(
				*outgoing_para,
				index,
			));
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
	) -> Result<(), QueueDownwardMessageError> {
		// Check if message is oversized.
		let serialized_len = msg.len() as u32;
		if serialized_len > config.max_downward_message_size {
			return Err(QueueDownwardMessageError::ExceedsMaxMessageSize)
		}

		let head = Wrapping(Self::dmp_queue_head(para));
		let mut tail = Wrapping(Self::dmp_queue_tail(para));
		// Get first/last message indexes.
		let message_idx = Self::dmp_message_idx(para);
		let current_message_idx = Wrapping(message_idx.1) + Wrapping(1);

		// Ring buffer is empty.
		if head == tail {
			// Tail always points to the next unused fragment.
			tail += 1;
			Self::update_tail(&para, tail);
		}

		let tail_fragment_len = <Self as Store>::DownwardMessageQueueFragments::decode_len(
			QueueFragmentIdx(para, (tail - Wrapping(1)).0),
		)
		.unwrap_or(0)
		.saturated_into::<u32>();

		// Check if we need a new fragment.
		if tail_fragment_len >= QUEUE_FRAGMENT_CAPACITY {
			// In practice this is always bounded economically.
			if tail + Wrapping(1) == head {
				unimplemented!("The end of the world is upon us");
			}

			// Advance tail.
			tail += 1;
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
				QueueMessageId(para, current_message_idx.0),
				|head| *head = new_head,
			);
		});

		// Insert message in the tail queue fragment.
		<Self as Store>::DownwardMessageQueueFragments::mutate(
			QueueFragmentIdx(para, (tail - Wrapping(1)).0),
			|v| {
				v.push(inbound);
			},
		);

		Self::update_tail(&para, tail);
		Self::update_last_message_idx(&para, current_message_idx);

		Ok(())
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

	/// Prunes the specified number of messages from the downward message queue of the given para.
	pub(crate) fn prune_dmq(para: ParaId, processed_downward_messages: u32) -> Weight {
		let mut head = Wrapping(Self::dmp_queue_head(para));
		let tail = Wrapping(Self::dmp_queue_tail(para));
		let mut messages_to_prune = processed_downward_messages as u64;
		let mut first_message_idx = Wrapping(Self::dmp_message_idx(para).0);
		let mut total_weight = T::DbWeight::get().reads_writes(3, 0);

		// TODO: also remove `DownwardMessageQueueHeadsById` entries.
		let mut keys_to_remove = Vec::new();

		// Determine the keys to be removed.
		while messages_to_prune > 0 && head != tail {
			let key = QueueFragmentIdx(para, head.0);
			<Self as Store>::DownwardMessageQueueFragments::mutate(key.clone(), |q| {
				let messages_in_fragment = q.len() as u64;
				
				if messages_to_prune >= messages_in_fragment {
					messages_to_prune = messages_to_prune.saturating_sub(messages_in_fragment);

					// Add mutata/removal weight. Removal happens later.
					total_weight += T::DbWeight::get().reads_writes(1, 2);
					keys_to_remove.push(key);

					// Advance first message index.
					first_message_idx += messages_in_fragment;

					// Advance head.
					head += 1;
				} else {
					first_message_idx += messages_to_prune ;
					*q = q.split_off(messages_to_prune as usize);

					// Only account for the update. This key will not be deleted.
					total_weight += T::DbWeight::get().reads_writes(1, 1);
					
					// Break loop.
					messages_to_prune = 0;
				}
			});
		}

		let _ = keys_to_remove
			.into_iter()
			.map(|k| <Self as Store>::DownwardMessageQueueFragments::remove(k));

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
		let mut head = Wrapping(Self::dmp_queue_head(para));
		let tail = Wrapping(Self::dmp_queue_tail(para));
		let mut length = 0;

		while head != tail {
			length += <Self as Store>::DownwardMessageQueueFragments::decode_len(QueueFragmentIdx(
				para, head.0,
			))
			.unwrap_or(0)
			.saturated_into::<u32>();
			head += 1;
		}

		length
	}

	/// Deprecated API. Please use `dmq_contents_bounded`.
	pub(crate) fn dmq_contents(recipient: ParaId) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		Self::dmq_contents_bounded(recipient, 0, MAX_MESSAGES_PER_QUERY)
	}

	/// Returns `count` messages starting from the index specified by `start`.
	/// The result will be an empty vector if the para doesn't exist or it's queue is empty.
	/// The result preserves the original message ordering - first element of vec is oldest message, while last is most
	/// recent.
	pub(crate) fn dmq_contents_bounded(
		recipient: ParaId,
		start: u32,
		count: u32,
	) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		let count = count as usize;
		let mut head = Wrapping(Self::dmp_queue_head(recipient));
		let tail = Wrapping(Self::dmp_queue_tail(recipient));
		let mut result = Vec::new();
		let mut to_skip = start;

		// Skip `start` messages.
		// TODO: switch interface to pages, so we can get rid of this skipping.
		while head != tail && to_skip > 0 {
			let fragment_len = <Self as Store>::DownwardMessageQueueFragments::decode_len(
				QueueFragmentIdx(recipient, head.0),
			)
			.unwrap_or(0)
			.saturated_into::<u32>();

			if to_skip < fragment_len {
				// We're gonna stop at this head after we add the remaining elements from this fragment to
				// the result set.
				result.extend(
					<Self as Store>::DownwardMessageQueueFragments::get(QueueFragmentIdx(
						recipient, head.0,
					))
					.split_off(to_skip as usize),
				);
			}

			to_skip = to_skip.saturating_sub(fragment_len);
			head += 1;
		}

		// Loop until we reach the tail, or we've gathered at least `count` messages.
		while head != tail && result.len() < count {
			result.extend(<Self as Store>::DownwardMessageQueueFragments::get(QueueFragmentIdx(
				recipient, head.0,
			)));

			// Advance to next fragment.
			head += 1;
		}

		// Clamp result as the simple loop above just accumulates all messages from fragments in `result`.
		if result.len() > count {
			let _ = result.split_off(count as usize);
		}

		result
	}
}
