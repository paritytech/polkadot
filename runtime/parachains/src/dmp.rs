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

//! This is a low level runtime component that manages the downward message queue for each
//! parachain. Messages are stored on the relay chain until they are processed by destination
//! parachains.
//!
//! The methods exposed here allow extending, reading and pruning of the message queue.
//!
//! Message queue storage format:
//! - The messages are queued in a ring buffer. There is a 1:1 mapping between a queue and
//! each parachain.
//! - The ring buffer is split in slots which we'll call pages. The pages can store up to
//! `QUEUE_PAGE_CAPACITY` messages.
//!
//! When sending messages, higher level code calls the `queue_downward_message` method which only fails
//! if the message size is higher than what the configuration defines in `max_downward_message_size`.
//! For every message sent we assign a sequential index and we store it for the first and last messages
//! in the queue.
//!
//! When a parachain consumes messages, they'll need a way to ensure the messages, or their ordering
//! were not altered in any way. A message queue chain(MQC) solves this as long as the last processed
//! head hash is available to the parachain. After sequentially hashing a subset of messages from
//! the message queue (tipically up to a certain weight), the parachain should arrive at the same MQC
//! head as the one provided by the relay chain.
//! This is implemented as a mapping between the message index and the MQC head for any given para.
//! That being said, parachains runtimes should also track the message indexes to access the MQC storage
//! proof.
//!

use crate::{
	configuration::{self, HostConfiguration},
	initializer,
};

use frame_support::pallet_prelude::*;
use primitives::v2::{DownwardMessage, Hash, Id as ParaId, InboundDownwardMessage};
use sp_runtime::traits::{BlakeTwo256, Hash as HashT, SaturatedConversion};
use sp_std::{fmt, prelude::*};
use xcm::latest::SendError;

pub use pallet::*;

#[cfg(test)]
mod tests;

pub mod migration;
pub mod ringbuf;
pub use ringbuf::*;

/// The state of the queue split in two sub-states, the ring bufer and the message window.
///
/// Invariants - see `RingBufferState` and `MessageWindowState`.
#[derive(Encode, Decode, Default, Clone, Copy, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct QueueState {
	ring_buffer_state: RingBufferState,
	message_window_state: MessageWindowState,
}

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

	/// A mapping between parachains and their message queue state.
	#[pallet::storage]
	#[pallet::getter(fn dmp_queue_state)]
	pub(super) type DownwardMessageQueueState<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, QueueState, ValueQuery>;

	/// A mapping between the queue pages of a parachain and the messages stored in it.
	///
	/// Invariants:
	/// - the vec is non-empty for any `QueuePageIdx`  in [head_page_idx, tail_page_idx).
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

	/// A mapping between a message and the corresponding MQC head hash.
	///
	/// Invariants:
	/// - the storage value is valid for any valid `MessageIdx`. Valid index means
	/// that the specified it is in `[first_message_idx, last_message_idx]`
	#[pallet::storage]
	pub(crate) type DownwardMessageQueueHeadsById<T: Config> =
		StorageMap<_, Twox64Concat, MessageIdx, Hash, ValueQuery>;

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

	pub(crate) fn update_state(para: &ParaId, new_state: QueueState) -> Weight {
		<Self as Store>::DownwardMessageQueueState::mutate(para, |state| {
			*state = new_state;
		});

		T::DbWeight::get().reads_writes(1, 1)
	}

	/// Remove all relevant storage items for an outgoing parachain.
	fn clean_dmp_after_outgoing(outgoing_para: &ParaId) {
		let state = Self::dmp_queue_state(outgoing_para);

		for page_idx in RingBuffer::with_state(state.ring_buffer_state, *outgoing_para) {
			<Self as Store>::DownwardMessageQueuePages::remove(page_idx);
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
		let QueueState { ring_buffer_state, message_window_state } = Self::dmp_queue_state(para);
		weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 0));

		let mut ring_buf = RingBuffer::with_state(ring_buffer_state, para);
		let mut message_window = MessageWindow::with_state(message_window_state, para);

		// Check if we need a new page.
		let mut page_idx = if ring_buf.size() == 0 {
			// Get a fresh page.
			ring_buf.extend()
		} else {
			// We've checked the queue is not empty, so `last_used` is guaranteed `Some`.
			// This page might be full but we check that later.
			ring_buf.last_used().unwrap()
		};

		let last_used_len = <Self as Store>::DownwardMessageQueuePages::decode_len(page_idx)
			.unwrap_or(0)
			.saturated_into::<u32>();

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 0));

		// Check if the page is full.
		if last_used_len >= QUEUE_PAGE_CAPACITY {
			// Get a fresh page.
			page_idx = ring_buf.extend()
		}

		let inbound =
			InboundDownwardMessage { msg, sent_at: <frame_system::Pallet<T>>::block_number() };

		// Obtain the new link in the MQC and update the head.
		<Self as Store>::DownwardMessageQueueHeads::mutate(para, |head| {
			let new_head =
				BlakeTwo256::hash_of(&(*head, inbound.sent_at, T::Hashing::hash_of(&inbound.msg)));
			*head = new_head;

			// Extend the message window by `1` message get it's index.
			new_message_idx = message_window.extend(1);

			// Update the head for the current message.
			<Self as Store>::DownwardMessageQueueHeadsById::mutate(new_message_idx, |head| {
				*head = new_head
			});
		});

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(2, 2));

		// Insert message in the tail queue page.
		<Self as Store>::DownwardMessageQueuePages::mutate(page_idx, |v| {
			v.push(inbound);
		});

		let ring_buffer_state = ring_buf.into_inner();
		let message_window_state = message_window.into_inner();
		Self::update_state(&para, QueueState { ring_buffer_state, message_window_state });

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

	/// MQC head key generator. Useful for pruning entries. Returns `count` MQC head mapping keys
	/// of the messages starting at index `start` for a given parachain.
	///
	/// Caller must ensure the indexes return are valid in the context of the `MessageWindow`.
	fn mqc_head_key_range(para: ParaId, start: WrappingIndex, count: u64) -> Vec<MessageIdx> {
		let mut keys = Vec::new();
		let mut idx = start;
		while idx != start.wrapping_add(count.into()) {
			keys.push(MessageIdx { para_id: para, message_idx: idx });
			idx = idx.wrapping_inc();
		}

		keys
	}

	/// Prunes the specified number of messages from the downward message queue of the given para.
	pub(crate) fn prune_dmq(para: ParaId, processed_downward_messages: u32) -> Weight {
		let QueueState { ring_buffer_state, message_window_state } = Self::dmp_queue_state(para);
		let mut ring_buf = RingBuffer::with_state(ring_buffer_state, para);
		let mut message_window = MessageWindow::with_state(message_window_state, para);

		let mut messages_to_prune = processed_downward_messages as u64;
		let mut total_weight = T::DbWeight::get().reads_writes(1, 0);

		let mut queue_keys_to_remove = Vec::new();
		let mut mqc_keys_to_remove = Vec::new();

		while messages_to_prune > 0 {
			if let Some(first_used_page) = ring_buf.front() {
				<Self as Store>::DownwardMessageQueuePages::mutate(first_used_page, |q| {
					let messages_in_page = q.len() as u64;

					// Add mutate weight. Removal happens later.
					total_weight += T::DbWeight::get().reads_writes(1, 1);

					if messages_to_prune >= messages_in_page {
						messages_to_prune = messages_to_prune.saturating_sub(messages_in_page);

						// Generate MQC head key for the messages in this page
						mqc_keys_to_remove.extend(Self::mqc_head_key_range(
							para,
							// Guaranteed to not panic, see invariants.
							message_window.first().unwrap().message_idx,
							messages_in_page,
						));

						message_window.prune(messages_in_page);
						// Queue this page for deletion from storage.
						queue_keys_to_remove.push(first_used_page);
						// Free the ring buffer page.
						ring_buf.pop_front();
					} else {
						mqc_keys_to_remove.extend(Self::mqc_head_key_range(
							para,
							message_window.first().unwrap().message_idx,
							messages_to_prune,
						));

						message_window.prune(messages_to_prune);
						*q = q.split_off(messages_to_prune as usize);

						// Break loop.
						messages_to_prune = 0;
					}
				});
			} else {
				// Queue is empty.
				break
			}
		}

		total_weight += T::DbWeight::get()
			.reads_writes(0, (queue_keys_to_remove.len() + mqc_keys_to_remove.len()) as u64);

		// Actually do cleanup.
		for key in queue_keys_to_remove {
			<Self as Store>::DownwardMessageQueuePages::remove(key);
		}

		for key in mqc_keys_to_remove {
			<Self as Store>::DownwardMessageQueueHeadsById::remove(key);
		}

		let ring_buffer_state = ring_buf.into_inner();
		let message_window_state = message_window.into_inner();
		Self::update_state(&para, QueueState { ring_buffer_state, message_window_state });
		total_weight += T::DbWeight::get().reads_writes(0, 1);

		total_weight
	}

	/// Returns the Head of Message Queue Chain for the given para or `None` if there is none
	/// associated with it.
	#[cfg(test)]
	fn dmq_mqc_head(para: ParaId) -> Hash {
		<Self as Store>::DownwardMessageQueueHeads::get(&para)
	}

	#[cfg(test)]
	fn dmq_mqc_head_for_message(para_id: ParaId, message_idx: WrappingIndex) -> Hash {
		<Self as Store>::DownwardMessageQueueHeadsById::get(&MessageIdx { para_id, message_idx })
	}

	/// Returns the number of pending downward messages addressed to the given para.
	///
	/// Returns 0 if the para doesn't have an associated downward message queue.
	pub(crate) fn dmq_length(para: ParaId) -> u32 {
		let state = Self::dmp_queue_state(para);
		MessageWindow::with_state(state.message_window_state, para).size() as u32
	}

	/// Deprecated API. Please use `dmq_contents_bounded`.
	pub(crate) fn dmq_contents(recipient: ParaId) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		Self::dmq_contents_bounded(recipient, 0, MAX_PAGES_PER_QUERY)
	}

	/// Get a subset of inbound messages from the downward message queue of a parachain.
	///
	/// Returns a `vec` containing the messages from the first `count` pages, starting from a `0` based
	/// page index specified by `start_page` with `0` being the first used page of the queue. A page
	/// can hold up to `QUEUE_PAGE_CAPACITY` messages.
	///
	/// Only the first and last pages of the queue can have less than maximum messages because insertion and
	/// pruning work with individual messages.
	///
	/// The result will be an empty vector if `count` is 0, the para doesn't exist, it's queue is empty
	/// or `start` is greater than the last used page in the queue. If the queue is not empty, the method
	/// is guaranteed to return at least 1 message and up to `count`*`QUEUE_PAGE_CAPACITY` messages.
	pub(crate) fn dmq_contents_bounded(
		recipient: ParaId,
		start: u32,
		count: u32,
	) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
		let state = Self::dmp_queue_state(recipient);
		let mut ring_buf = RingBuffer::with_state(state.ring_buffer_state, recipient);

		// Skip first `start` pages.
		ring_buf.prune(start);

		let mut result = Vec::new();
		let mut pages_fetched = 0;

		for page_idx in ring_buf {
			if count == pages_fetched {
				break
			}
			result.extend(<Self as Store>::DownwardMessageQueuePages::get(page_idx));
			pages_fetched += 1;
		}

		result
	}
}
