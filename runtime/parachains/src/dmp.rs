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
use primitives::v2::{DownwardMessage, Hash, Id as ParaId, InboundDownwardMessage};
use sp_runtime::traits::{BlakeTwo256, Hash as HashT, SaturatedConversion};
use sp_std::{fmt, num::Wrapping, prelude::*};
use xcm::latest::SendError;

pub use pallet::*;

#[cfg(test)]
mod tests;

/// The message key for a group of downward messages.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct QueueFragmentId(ParaId, u8);

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

/// A slice of the `para dmq`.
pub type QueueFragment<T: Config> = Vec<InboundDownwardMessage<T::BlockNumber>>;

/// To reduce the runtime memory footprint when sending or receiving messages we will split
/// the queue in fragments of `QUEUE_FRAGMENT_SIZE` capacity. Tuning this constant allows
/// to control how we trade off the overhead per stored message vs memory footprint of individual
/// message read/writes from/to storage. The fragments are part of circular buffer per para and
/// we keep track of the head and tail fragments.
///
/// TODO(maybe) - make these configuration parameters?
///
/// Defines the queue fragment capacity.
pub const QUEUE_FRAGMENT_SIZE: u32 = 32;
/// Defines the maximum amount of fragments to process when calling `dmq_contents`.
pub const MAX_FRAGMENTS_PER_QUERY: u32 = 8;

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
		StorageMap<_, Twox64Concat, ParaId, u8, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn dmp_queue_tail)]
	pub(super) type DownwardMessageQueueTail<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, u8, ValueQuery>;

	/// The downward messages addressed for a certain para.
	#[pallet::storage]
	pub(crate) type DownwardMessageQueues<T: Config> =
		StorageMap<_, Twox64Concat, QueueFragmentId, QueueFragment<T>, ValueQuery>;

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

	fn update_head(para: &ParaId, new_head: Wrapping<u8>) -> Weight {
		<Self as Store>::DownwardMessageQueueHead::mutate(para, |head_pointer| {
			*head_pointer = new_head.0;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	fn update_tail(para: &ParaId, new_tail: Wrapping<u8>) -> Weight {
		<Self as Store>::DownwardMessageQueueTail::mutate(para, |tail_pointer| {
			*tail_pointer = new_tail.0;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	/// Remove all relevant storage items for an outgoing parachain.
	fn clean_dmp_after_outgoing(outgoing_para: &ParaId) {
		for index in 0..u8::MAX {
			<Self as Store>::DownwardMessageQueues::remove(QueueFragmentId(*outgoing_para, index));
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

		// Ring buffer is empty.
		if head == tail {
			// Tail always points to the next buffer unused fragment.
			tail += 1;
			Self::update_tail(&para, tail);
		}

		let tail_fragment_len = <Self as Store>::DownwardMessageQueues::decode_len(
			QueueFragmentId(para, (tail - Wrapping(1)).0),
		)
		.unwrap_or(0)
		.saturated_into::<u32>();

		// Check if we need a new fragment.
		if tail_fragment_len >= QUEUE_FRAGMENT_SIZE {
			// Check if ring buffer is full.
			if tail + std::num::Wrapping(1) == head {
				return Err(QueueDownwardMessageError::ExceedsMaxPendingMessageCount)
			}

			// Advance tail.
			tail += 1;
			Self::update_tail(&para, tail);
		}

		let inbound =
			InboundDownwardMessage { msg, sent_at: <frame_system::Pallet<T>>::block_number() };

		// obtain the new link in the MQC and update the head.
		<Self as Store>::DownwardMessageQueueHeads::mutate(para, |head| {
			let new_head =
				BlakeTwo256::hash_of(&(*head, inbound.sent_at, T::Hashing::hash_of(&inbound.msg)));
			*head = new_head;
		});

		// Insert message in the tail queue fragment.
		<Self as Store>::DownwardMessageQueues::mutate(
			QueueFragmentId(para, (tail - Wrapping(1)).0),
			|v| {
				v.push(inbound);
			},
		);

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
		let mut head = std::num::Wrapping(Self::dmp_queue_head(para));
		let tail = std::num::Wrapping(Self::dmp_queue_tail(para));
		let mut messages_to_prune = processed_downward_messages as usize;
		let mut total_weight = T::DbWeight::get().reads_writes(2, 0);

		// Prune all processed messages in multiple fragments in the ring buffer.
		while messages_to_prune > 0 && head != tail {
			<Self as Store>::DownwardMessageQueues::mutate(QueueFragmentId(para, head.0), |q| {
				if messages_to_prune > q.len() {
					messages_to_prune = messages_to_prune.saturating_sub(q.len());
					q.clear();
					// Advance head.
					head += 1;
				} else {
					*q = q.split_off(messages_to_prune);
					messages_to_prune = 0;
				}
			});
			total_weight += T::DbWeight::get().reads_writes(1, 1);
		}

		// Update head.
		Self::update_head(&para, head) + total_weight
	}

	/// Returns the Head of Message Queue Chain for the given para or `None` if there is none
	/// associated with it.
	#[cfg(test)]
	fn dmq_mqc_head(para: ParaId) -> Hash {
		<Self as Store>::DownwardMessageQueueHeads::get(&para)
	}

	/// Returns the number of pending downward messages addressed to the given para.
	///
	/// Returns 0 if the para doesn't have an associated downward message queue.
	pub(crate) fn dmq_length(para: ParaId) -> u32 {
		let mut head = std::num::Wrapping(Self::dmp_queue_head(para));
		let tail = std::num::Wrapping(Self::dmp_queue_tail(para));
		let mut length = 0;

		while head != tail {
			length +=
				<Self as Store>::DownwardMessageQueues::decode_len(QueueFragmentId(para, head.0))
					.unwrap_or(0)
					.saturated_into::<u32>();
			head += 1;
		}

		length
	}

	/// Returns up to `MAX_FRAGMENTS_PER_QUERY*QUEUE_FRAGMENT_SIZE` messages from the queue contents for the given para.
	/// The result will be an empty vector if the para doesn't exist or it's queue is empty.
	/// The result preserves the original message ordering - first element of vec is oldest message, while last is most
	/// recent.
	/// This is to be used in conjuction with `prune_dmq` to achieve pagination of arbitrary large queues.
	pub(crate) fn dmq_contents(recipient: ParaId) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		let mut head = std::num::Wrapping(Self::dmp_queue_head(recipient));
		let tail = std::num::Wrapping(Self::dmp_queue_tail(recipient));
		let mut result = Vec::new();

		while head != tail &&
			result.len() <= (MAX_FRAGMENTS_PER_QUERY * QUEUE_FRAGMENT_SIZE) as usize
		{
			result.extend(<Self as Store>::DownwardMessageQueues::get(QueueFragmentId(
				recipient, head.0,
			)));
			// Advance to next fragment.
			head += 1;
		}

		// Clamp result as the simple logic above just accumulates all messages from fragments.
		let _ = result.split_off((MAX_FRAGMENTS_PER_QUERY * QUEUE_FRAGMENT_SIZE) as usize);

		result
	}
}
