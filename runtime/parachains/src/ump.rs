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
use frame_support::{pallet_prelude::*, traits::EnsureOrigin};
use frame_system::pallet_prelude::*;
use primitives::v1::{Id as ParaId, UpwardMessage};
use sp_std::{
	collections::btree_map::BTreeMap, convert::TryFrom, fmt, marker::PhantomData, mem, prelude::*,
};
use xcm::latest::Outcome;

pub use pallet::*;

/// All upward messages coming from parachains will be funneled into an implementation of this trait.
///
/// The message is opaque from the perspective of UMP. The message size can range from 0 to
/// `config.max_upward_message_size`.
///
/// It's up to the implementation of this trait to decide what to do with a message as long as it
/// returns the amount of weight consumed in the process of handling. Ignoring a message is a valid
/// strategy.
///
/// There are no guarantees on how much time it takes for the message sent by a candidate to end up
/// in the sink after the candidate was enacted. That typically depends on the UMP traffic, the sizes
/// of upward messages and the configuration of UMP.
///
/// It is possible that by the time the message is sank the origin parachain was offboarded. It is
/// up to the implementer to check that if it cares.
pub trait UmpSink {
	/// Process an incoming upward message and return the amount of weight it consumed, or `None` if
	/// it did not begin processing a message since it would otherwise exceed `max_weight`.
	///
	/// See the trait docs for more details.
	fn process_upward_message(
		origin: ParaId,
		msg: &[u8],
		max_weight: Weight,
	) -> Result<Weight, (MessageId, Weight)>;
}

/// An implementation of a sink that just swallows the message without consuming any weight. Returns
/// `Some(0)` indicating that no messages existed for it to process.
impl UmpSink for () {
	fn process_upward_message(
		_: ParaId,
		_: &[u8],
		_: Weight,
	) -> Result<Weight, (MessageId, Weight)> {
		Ok(0)
	}
}

/// Simple type used to identify messages for the purpose of reporting events. Secure if and only
/// if the message content is unique.
pub type MessageId = [u8; 32];

/// Index used to identify overweight messages.
pub type OverweightIndex = u64;

/// A specific implementation of a `UmpSink` where messages are in the XCM format
/// and will be forwarded to the XCM Executor.
pub struct XcmSink<XcmExecutor, Config>(PhantomData<(XcmExecutor, Config)>);

/// Returns a [`MessageId`] for the given upward message payload.
fn upward_message_id(data: &[u8]) -> MessageId {
	sp_io::hashing::blake2_256(data)
}

impl<XcmExecutor: xcm::latest::ExecuteXcm<C::Call>, C: Config> UmpSink for XcmSink<XcmExecutor, C> {
	fn process_upward_message(
		origin: ParaId,
		data: &[u8],
		max_weight: Weight,
	) -> Result<Weight, (MessageId, Weight)> {
		use parity_scale_codec::DecodeLimit;
		use xcm::{
			latest::{Error as XcmError, Junction, Xcm},
			VersionedXcm,
		};

		let id = upward_message_id(&data[..]);
		let maybe_msg = VersionedXcm::<C::Call>::decode_all_with_depth_limit(
			xcm::MAX_XCM_DECODE_DEPTH,
			&mut &data[..],
		)
		.map(Xcm::<C::Call>::try_from);
		match maybe_msg {
			Err(_) => {
				Pallet::<C>::deposit_event(Event::InvalidFormat(id));
				Ok(0)
			},
			Ok(Err(())) => {
				Pallet::<C>::deposit_event(Event::UnsupportedVersion(id));
				Ok(0)
			},
			Ok(Ok(xcm_message)) => {
				let xcm_junction = Junction::Parachain(origin.into());
				let outcome = XcmExecutor::execute_xcm(xcm_junction, xcm_message, max_weight);
				match outcome {
					Outcome::Error(XcmError::WeightLimitReached(required)) => Err((id, required)),
					outcome => {
						let weight_used = outcome.weight_used();
						Pallet::<C>::deposit_event(Event::ExecutedUpward(id, outcome));
						Ok(weight_used)
					},
				}
			},
		}
	}
}

/// An error returned by [`check_upward_messages`] that indicates a violation of one of acceptance
/// criteria rules.
pub enum AcceptanceCheckErr {
	MoreMessagesThanPermitted { sent: u32, permitted: u32 },
	MessageSize { idx: u32, msg_size: u32, max_size: u32 },
	CapacityExceeded { count: u32, limit: u32 },
	TotalSizeExceeded { total_size: u32, limit: u32 },
}

impl fmt::Debug for AcceptanceCheckErr {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			AcceptanceCheckErr::MoreMessagesThanPermitted { sent, permitted } => write!(
				fmt,
				"more upward messages than permitted by config ({} > {})",
				sent, permitted,
			),
			AcceptanceCheckErr::MessageSize { idx, msg_size, max_size } => write!(
				fmt,
				"upward message idx {} larger than permitted by config ({} > {})",
				idx, msg_size, max_size,
			),
			AcceptanceCheckErr::CapacityExceeded { count, limit } => write!(
				fmt,
				"the ump queue would have more items than permitted by config ({} > {})",
				count, limit,
			),
			AcceptanceCheckErr::TotalSizeExceeded { total_size, limit } => write!(
				fmt,
				"the ump queue would have grown past the max size permitted by config ({} > {})",
				total_size, limit,
			),
		}
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config {
		/// The aggregate event.
		type Event: From<Event> + IsType<<Self as frame_system::Config>::Event>;

		/// A place where all received upward messages are funneled.
		type UmpSink: UmpSink;

		/// The factor by which the weight limit it multiplied for the first UMP message to execute with.
		///
		/// An amount less than 100 keeps more available weight in the queue for messages after the first, and potentially
		/// stalls the queue in doing so. More than 100 will provide additional weight for the first message only.
		///
		/// Generally you'll want this to be a bit more - 150 or 200 would be good values.
		type FirstMessageFactorPercent: Get<Weight>;

		/// Origin which is allowed to execute overweight messages.
		type ExecuteOverweightOrigin: EnsureOrigin<Self::Origin>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event {
		/// Upward message is invalid XCM.
		/// \[ id \]
		InvalidFormat(MessageId),
		/// Upward message is unsupported version of XCM.
		/// \[ id \]
		UnsupportedVersion(MessageId),
		/// Upward message executed with the given outcome.
		/// \[ id, outcome \]
		ExecutedUpward(MessageId, Outcome),
		/// The weight limit for handling downward messages was reached.
		/// \[ id, remaining, required \]
		WeightExhausted(MessageId, Weight, Weight),
		/// Some downward messages have been received and will be processed.
		/// \[ para, count, size \]
		UpwardMessagesReceived(ParaId, u32, u32),
		/// The weight budget was exceeded for an individual downward message.
		///
		/// This message can be later dispatched manually using `service_overweight` dispatchable
		/// using the assigned `overweight_index`.
		///
		/// \[ para, id, overweight_index, required \]
		OverweightEnqueued(ParaId, MessageId, OverweightIndex, Weight),
		/// Downward message from the overweight queue was executed with the given actual weight
		/// used.
		///
		/// \[ overweight_index, used \]
		OverweightServiced(OverweightIndex, Weight),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The message index given is unknown.
		UnknownMessageIndex,
		/// The amount of weight given is possibly not enough for executing the message.
		WeightOverLimit,
	}

	/// The messages waiting to be handled by the relay-chain originating from a certain parachain.
	///
	/// Note that some upward messages might have been already processed by the inclusion logic. E.g.
	/// channel management messages.
	///
	/// The messages are processed in FIFO order.
	#[pallet::storage]
	pub type RelayDispatchQueues<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, Vec<UpwardMessage>, ValueQuery>;

	/// Size of the dispatch queues. Caches sizes of the queues in `RelayDispatchQueue`.
	///
	/// First item in the tuple is the count of messages and second
	/// is the total length (in bytes) of the message payloads.
	///
	/// Note that this is an auxiliary mapping: it's possible to tell the byte size and the number of
	/// messages only looking at `RelayDispatchQueues`. This mapping is separate to avoid the cost of
	/// loading the whole message queue if only the total size and count are required.
	///
	/// Invariant:
	/// - The set of keys should exactly match the set of keys of `RelayDispatchQueues`.
	// NOTE that this field is used by parachains via merkle storage proofs, therefore changing
	// the format will require migration of parachains.
	#[pallet::storage]
	pub type RelayDispatchQueueSize<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, (u32, u32), ValueQuery>;

	/// The ordered list of `ParaId`s that have a `RelayDispatchQueue` entry.
	///
	/// Invariant:
	/// - The set of items from this vector should be exactly the set of the keys in
	///   `RelayDispatchQueues` and `RelayDispatchQueueSize`.
	#[pallet::storage]
	pub type NeedsDispatch<T: Config> = StorageValue<_, Vec<ParaId>, ValueQuery>;

	/// This is the para that gets will get dispatched first during the next upward dispatchable queue
	/// execution round.
	///
	/// Invariant:
	/// - If `Some(para)`, then `para` must be present in `NeedsDispatch`.
	#[pallet::storage]
	pub type NextDispatchRoundStartWith<T: Config> = StorageValue<_, ParaId>;

	/// The messages that exceeded max individual message weight budget.
	///
	/// These messages stay there until manually dispatched.
	#[pallet::storage]
	pub type Overweight<T: Config> =
		StorageMap<_, Twox64Concat, OverweightIndex, (ParaId, Vec<u8>), OptionQuery>;

	/// The number of overweight messages ever recorded in `Overweight` (and thus the lowest free
	/// index).
	#[pallet::storage]
	pub type OverweightCount<T: Config> = StorageValue<_, OverweightIndex, ValueQuery>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Service a single overweight upward message.
		///
		/// - `origin`: Must pass `ExecuteOverweightOrigin`.
		/// - `index`: The index of the overweight message to service.
		/// - `weight_limit`: The amount of weight that message execution may take.
		///
		/// Errors:
		/// - `UnknownMessageIndex`: Message of `index` is unknown.
		/// - `WeightOverLimit`: Message execution may use greater than `weight_limit`.
		///
		/// Events:
		/// - `OverweightServiced`: On success.
		#[pallet::weight(weight_limit.saturating_add(1_000_000))]
		pub fn service_overweight(
			origin: OriginFor<T>,
			index: OverweightIndex,
			weight_limit: Weight,
		) -> DispatchResultWithPostInfo {
			T::ExecuteOverweightOrigin::ensure_origin(origin)?;

			let (sender, data) =
				Overweight::<T>::get(index).ok_or(Error::<T>::UnknownMessageIndex)?;
			let used = T::UmpSink::process_upward_message(sender, &data[..], weight_limit)
				.map_err(|_| Error::<T>::WeightOverLimit)?;
			Overweight::<T>::remove(index);
			Self::deposit_event(Event::OverweightServiced(index, used));
			Ok(Some(used.saturating_add(1_000_000)).into())
		}
	}
}

/// Routines related to the upward message passing.
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
			Self::clean_ump_after_outgoing(outgoing_para);
		}
	}

	/// Remove all relevant storage items for an outgoing parachain.
	fn clean_ump_after_outgoing(outgoing_para: &ParaId) {
		<Self as Store>::RelayDispatchQueueSize::remove(outgoing_para);
		<Self as Store>::RelayDispatchQueues::remove(outgoing_para);

		// Remove the outgoing para from the `NeedsDispatch` list and from
		// `NextDispatchRoundStartWith`.
		//
		// That's needed for maintaining invariant that `NextDispatchRoundStartWith` points to an
		// existing item in `NeedsDispatch`.
		<Self as Store>::NeedsDispatch::mutate(|v| {
			if let Ok(i) = v.binary_search(outgoing_para) {
				v.remove(i);
			}
		});
		<Self as Store>::NextDispatchRoundStartWith::mutate(|v| {
			*v = v.filter(|p| p == outgoing_para)
		});
	}

	/// Check that all the upward messages sent by a candidate pass the acceptance criteria. Returns
	/// false, if any of the messages doesn't pass.
	pub(crate) fn check_upward_messages(
		config: &HostConfiguration<T::BlockNumber>,
		para: ParaId,
		upward_messages: &[UpwardMessage],
	) -> Result<(), AcceptanceCheckErr> {
		if upward_messages.len() as u32 > config.max_upward_message_num_per_candidate {
			return Err(AcceptanceCheckErr::MoreMessagesThanPermitted {
				sent: upward_messages.len() as u32,
				permitted: config.max_upward_message_num_per_candidate,
			})
		}

		let (mut para_queue_count, mut para_queue_size) =
			<Self as Store>::RelayDispatchQueueSize::get(&para);

		for (idx, msg) in upward_messages.into_iter().enumerate() {
			let msg_size = msg.len() as u32;
			if msg_size > config.max_upward_message_size {
				return Err(AcceptanceCheckErr::MessageSize {
					idx: idx as u32,
					msg_size,
					max_size: config.max_upward_message_size,
				})
			}
			para_queue_count += 1;
			para_queue_size += msg_size;
		}

		// make sure that the queue is not overfilled.
		// we do it here only once since returning false invalidates the whole relay-chain block.
		if para_queue_count > config.max_upward_queue_count {
			return Err(AcceptanceCheckErr::CapacityExceeded {
				count: para_queue_count,
				limit: config.max_upward_queue_count,
			})
		}
		if para_queue_size > config.max_upward_queue_size {
			return Err(AcceptanceCheckErr::TotalSizeExceeded {
				total_size: para_queue_size,
				limit: config.max_upward_queue_size,
			})
		}

		Ok(())
	}

	/// Enqueues `upward_messages` from a `para`'s accepted candidate block.
	pub(crate) fn receive_upward_messages(
		para: ParaId,
		upward_messages: Vec<UpwardMessage>,
	) -> Weight {
		let mut weight = 0;

		if !upward_messages.is_empty() {
			let (extra_count, extra_size) = upward_messages
				.iter()
				.fold((0, 0), |(cnt, size), d| (cnt + 1, size + d.len() as u32));

			<Self as Store>::RelayDispatchQueues::mutate(&para, |v| {
				v.extend(upward_messages.into_iter())
			});

			<Self as Store>::RelayDispatchQueueSize::mutate(
				&para,
				|(ref mut cnt, ref mut size)| {
					*cnt += extra_count;
					*size += extra_size;
				},
			);

			<Self as Store>::NeedsDispatch::mutate(|v| {
				if let Err(i) = v.binary_search(&para) {
					v.insert(i, para);
				}
			});

			// NOTE: The actual computation is not accounted for. It should be benchmarked.
			weight += T::DbWeight::get().reads_writes(3, 3);

			Self::deposit_event(Event::UpwardMessagesReceived(para, extra_count, extra_size));
		}

		weight
	}

	/// Devote some time into dispatching pending upward messages.
	pub(crate) fn process_pending_upward_messages() -> Weight {
		let mut weight_used = 0;

		let config = <configuration::Pallet<T>>::config();
		let mut cursor = NeedsDispatchCursor::new::<T>();
		let mut queue_cache = QueueCache::new();

		while let Some(dispatchee) = cursor.peek() {
			if weight_used >= config.ump_service_total_weight {
				// Then check whether we've reached or overshoot the
				// preferred weight for the dispatching stage.
				//
				// if so - bail.
				break
			}
			let max_weight = if weight_used == 0 {
				// we increase the amount of weight that we're allowed to use on the first message to try to prevent
				// the possibility of blockage of the queue.
				config.ump_service_total_weight * T::FirstMessageFactorPercent::get() / 100
			} else {
				config.ump_service_total_weight - weight_used
			};

			// attempt to process the next message from the queue of the dispatchee; if not beyond
			// our remaining weight limit, then consume it.
			let maybe_next = queue_cache.peek_front::<T>(dispatchee);
			if let Some(upward_message) = maybe_next {
				match T::UmpSink::process_upward_message(dispatchee, upward_message, max_weight) {
					Ok(used) => {
						weight_used += used;
						let _ = queue_cache.consume_front::<T>(dispatchee);
					},
					Err((id, required)) => {
						if required > config.ump_max_individual_weight {
							// overweight - add to overweight queue and continue with message
							// execution consuming the message.
							let upward_message = queue_cache.consume_front::<T>(dispatchee).expect(
								"`consume_front` should return the same msg as `peek_front`;\
								if we get into this branch then `peek_front` returned `Some`;\
								thus `upward_message` cannot be `None`; qed",
							);
							let index = Self::stash_overweight(dispatchee, upward_message);
							Self::deposit_event(Event::OverweightEnqueued(
								dispatchee, id, index, required,
							));
						} else {
							// we process messages in order and don't drop them if we run out of weight,
							// so need to break here without calling `consume_front`.
							Self::deposit_event(Event::WeightExhausted(id, max_weight, required));
							break
						}
					},
				}
			}

			if queue_cache.is_empty::<T>(dispatchee) {
				// the queue is empty now - this para doesn't need attention anymore.
				cursor.remove();
			} else {
				cursor.advance();
			}
		}

		cursor.flush::<T>();
		queue_cache.flush::<T>();

		weight_used
	}

	/// Puts a given upward message into the list of overweight messages allowing it to be executed
	/// later.
	fn stash_overweight(sender: ParaId, upward_message: Vec<u8>) -> OverweightIndex {
		let index = <Self as Store>::OverweightCount::mutate(|count| {
			let index = *count;
			*count += 1;
			index
		});

		<Self as Store>::Overweight::insert(index, (sender, upward_message));
		index
	}
}

/// To avoid constant fetching, deserializing and serialization the queues are cached.
///
/// After an item dequeued from a queue for the first time, the queue is stored in this struct
/// rather than being serialized and persisted.
///
/// This implementation works best when:
///
/// 1. when the queues are shallow
/// 2. the dispatcher makes more than one cycle
///
/// if the queues are deep and there are many we would load and keep the queues for a long time,
/// thus increasing the peak memory consumption of the wasm runtime. Under such conditions persisting
/// queues might play better since it's unlikely that they are going to be requested once more.
///
/// On the other hand, the situation when deep queues exist and it takes more than one dispatcher
/// cycle to traverse the queues is already sub-optimal and better be avoided.
///
/// This struct is not supposed to be dropped but rather to be consumed by [`flush`].
struct QueueCache(BTreeMap<ParaId, QueueCacheEntry>);

struct QueueCacheEntry {
	queue: Vec<UpwardMessage>,
	total_size: u32,
	consumed_count: usize,
	consumed_size: usize,
}

impl QueueCache {
	fn new() -> Self {
		Self(BTreeMap::new())
	}

	fn ensure_cached<T: Config>(&mut self, para: ParaId) -> &mut QueueCacheEntry {
		self.0.entry(para).or_insert_with(|| {
			let queue = RelayDispatchQueues::<T>::get(&para);
			let (_, total_size) = RelayDispatchQueueSize::<T>::get(&para);
			QueueCacheEntry { queue, total_size, consumed_count: 0, consumed_size: 0 }
		})
	}

	/// Returns the message at the front of `para`'s queue, or `None` if the queue is empty.
	///
	/// Does not mutate the queue.
	fn peek_front<T: Config>(&mut self, para: ParaId) -> Option<&UpwardMessage> {
		let entry = self.ensure_cached::<T>(para);
		entry.queue.get(entry.consumed_count)
	}

	/// Attempts to remove one message from the front of `para`'s queue. If the queue is empty, then
	/// does nothing.
	fn consume_front<T: Config>(&mut self, para: ParaId) -> Option<UpwardMessage> {
		let cache_entry = self.ensure_cached::<T>(para);

		match cache_entry.queue.get_mut(cache_entry.consumed_count) {
			Some(msg) => {
				cache_entry.consumed_count += 1;
				cache_entry.consumed_size += msg.len();

				Some(mem::take(msg))
			},
			None => None,
		}
	}

	/// Returns if the queue for the given para is empty.
	///
	/// That is, if this returns `true` then the next call to [`peek_front`] will return `None`.
	///
	/// Does not mutate the queue.
	fn is_empty<T: Config>(&mut self, para: ParaId) -> bool {
		let cache_entry = self.ensure_cached::<T>(para);
		cache_entry.consumed_count >= cache_entry.queue.len()
	}

	/// Flushes the updated queues into the storage.
	fn flush<T: Config>(self) {
		// NOTE we use an explicit method here instead of Drop impl because it has unwanted semantics
		// within runtime. It is dangerous to use because of double-panics and flushing on a panic
		// is not necessary as well.
		for (para, entry) in self.0 {
			if entry.consumed_count >= entry.queue.len() {
				// remove the entries altogether.
				RelayDispatchQueues::<T>::remove(&para);
				RelayDispatchQueueSize::<T>::remove(&para);
			} else if entry.consumed_count > 0 {
				RelayDispatchQueues::<T>::insert(&para, &entry.queue[entry.consumed_count..]);
				let count = (entry.queue.len() - entry.consumed_count) as u32;
				let size = entry.total_size.saturating_sub(entry.consumed_size as u32);
				RelayDispatchQueueSize::<T>::insert(&para, (count, size));
			}
		}
	}
}

/// A cursor that iterates over all entries in `NeedsDispatch`.
///
/// This cursor will start with the para indicated by `NextDispatchRoundStartWith` storage entry.
/// This cursor is cyclic meaning that after reaching the end it will jump to the beginning. Unlike
/// an iterator, this cursor allows removing items during the iteration.
///
/// Each iteration cycle *must be* concluded with a call to either `advance` or `remove`.
///
/// This struct is not supposed to be dropped but rather to be consumed by [`flush`].
#[derive(Debug)]
struct NeedsDispatchCursor {
	needs_dispatch: Vec<ParaId>,
	index: usize,
}

impl NeedsDispatchCursor {
	fn new<T: Config>() -> Self {
		let needs_dispatch: Vec<ParaId> = <Pallet<T> as Store>::NeedsDispatch::get();
		let start_with = <Pallet<T> as Store>::NextDispatchRoundStartWith::get();

		let initial_index = match start_with {
			Some(para) => match needs_dispatch.binary_search(&para) {
				Ok(found_index) => found_index,
				Err(_supposed_index) => {
					// well that's weird because we maintain an invariant that
					// `NextDispatchRoundStartWith` must point into one of the items in
					// `NeedsDispatch`.
					//
					// let's select 0 as the starting index as a safe bet.
					debug_assert!(false);
					0
				},
			},
			None => 0,
		};

		Self { needs_dispatch, index: initial_index }
	}

	/// Returns the item the cursor points to.
	fn peek(&self) -> Option<ParaId> {
		self.needs_dispatch.get(self.index).cloned()
	}

	/// Moves the cursor to the next item.
	fn advance(&mut self) {
		if self.needs_dispatch.is_empty() {
			return
		}
		self.index = (self.index + 1) % self.needs_dispatch.len();
	}

	/// Removes the item under the cursor.
	fn remove(&mut self) {
		if self.needs_dispatch.is_empty() {
			return
		}
		let _ = self.needs_dispatch.remove(self.index);

		// we might've removed the last element and that doesn't necessarily mean that `needs_dispatch`
		// became empty. Reposition the cursor in this case to the beginning.
		if self.needs_dispatch.get(self.index).is_none() {
			self.index = 0;
		}
	}

	/// Flushes the dispatcher state into the persistent storage.
	fn flush<T: Config>(self) {
		let next_one = self.peek();
		<Pallet<T> as Store>::NextDispatchRoundStartWith::set(next_one);
		<Pallet<T> as Store>::NeedsDispatch::put(self.needs_dispatch);
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use crate::mock::{
		assert_last_event, new_test_ext, take_processed, Configuration, MockGenesisConfig, Origin,
		System, Test, Ump,
	};
	use frame_support::{assert_noop, assert_ok, weights::Weight};
	use std::collections::HashSet;

	struct GenesisConfigBuilder {
		max_upward_message_size: u32,
		max_upward_message_num_per_candidate: u32,
		max_upward_queue_count: u32,
		max_upward_queue_size: u32,
		ump_service_total_weight: Weight,
		ump_max_individual_weight: Weight,
	}

	impl Default for GenesisConfigBuilder {
		fn default() -> Self {
			Self {
				max_upward_message_size: 16,
				max_upward_message_num_per_candidate: 2,
				max_upward_queue_count: 4,
				max_upward_queue_size: 64,
				ump_service_total_weight: 1000,
				ump_max_individual_weight: 100,
			}
		}
	}

	impl GenesisConfigBuilder {
		fn build(self) -> crate::mock::MockGenesisConfig {
			let mut genesis = default_genesis_config();
			let config = &mut genesis.configuration.config;

			config.max_upward_message_size = self.max_upward_message_size;
			config.max_upward_message_num_per_candidate = self.max_upward_message_num_per_candidate;
			config.max_upward_queue_count = self.max_upward_queue_count;
			config.max_upward_queue_size = self.max_upward_queue_size;
			config.ump_service_total_weight = self.ump_service_total_weight;
			config.ump_max_individual_weight = self.ump_max_individual_weight;
			genesis
		}
	}

	fn default_genesis_config() -> MockGenesisConfig {
		MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: crate::configuration::HostConfiguration {
					max_downward_message_size: 1024,
					..Default::default()
				},
			},
			..Default::default()
		}
	}

	fn queue_upward_msg(para: ParaId, msg: UpwardMessage) {
		let msgs = vec![msg];
		assert!(Ump::check_upward_messages(&Configuration::config(), para, &msgs).is_ok());
		let _ = Ump::receive_upward_messages(para, msgs);
	}

	fn assert_storage_consistency_exhaustive() {
		// check that empty queues don't clutter the storage.
		for (_para, queue) in <Ump as Store>::RelayDispatchQueues::iter() {
			assert!(!queue.is_empty());
		}

		// actually count the counts and sizes in queues and compare them to the bookkeeped version.
		for (para, queue) in <Ump as Store>::RelayDispatchQueues::iter() {
			let (expected_count, expected_size) = <Ump as Store>::RelayDispatchQueueSize::get(para);
			let (actual_count, actual_size) =
				queue.into_iter().fold((0, 0), |(acc_count, acc_size), x| {
					(acc_count + 1, acc_size + x.len() as u32)
				});

			assert_eq!(expected_count, actual_count);
			assert_eq!(expected_size, actual_size);
		}

		// since we wipe the empty queues the sets of paras in queue contents, queue sizes and
		// need dispatch set should all be equal.
		let queue_contents_set = <Ump as Store>::RelayDispatchQueues::iter()
			.map(|(k, _)| k)
			.collect::<HashSet<ParaId>>();
		let queue_sizes_set = <Ump as Store>::RelayDispatchQueueSize::iter()
			.map(|(k, _)| k)
			.collect::<HashSet<ParaId>>();
		let needs_dispatch_set =
			<Ump as Store>::NeedsDispatch::get().into_iter().collect::<HashSet<ParaId>>();
		assert_eq!(queue_contents_set, queue_sizes_set);
		assert_eq!(queue_contents_set, needs_dispatch_set);

		// `NextDispatchRoundStartWith` should point into a para that is tracked.
		if let Some(para) = <Ump as Store>::NextDispatchRoundStartWith::get() {
			assert!(queue_contents_set.contains(&para));
		}

		// `NeedsDispatch` is always sorted.
		assert!(<Ump as Store>::NeedsDispatch::get().windows(2).all(|xs| xs[0] <= xs[1]));
	}

	#[test]
	fn dispatch_empty() {
		new_test_ext(default_genesis_config()).execute_with(|| {
			assert_storage_consistency_exhaustive();

			// make sure that the case with empty queues is handled properly
			Ump::process_pending_upward_messages();

			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn dispatch_single_message() {
		let a = ParaId::from(228);
		let msg = 1000u32.encode();

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			queue_upward_msg(a, msg.clone());
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(a, msg)]);

			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn dispatch_resume_after_exceeding_dispatch_stage_weight() {
		let a = ParaId::from(128);
		let c = ParaId::from(228);
		let q = ParaId::from(911);

		let a_msg_1 = (200u32, "a_msg_1").encode();
		let a_msg_2 = (100u32, "a_msg_2").encode();
		let c_msg_1 = (300u32, "c_msg_1").encode();
		let c_msg_2 = (100u32, "c_msg_2").encode();
		let q_msg = (500u32, "q_msg").encode();

		new_test_ext(
			GenesisConfigBuilder { ump_service_total_weight: 500, ..Default::default() }.build(),
		)
		.execute_with(|| {
			queue_upward_msg(q, q_msg.clone());
			queue_upward_msg(c, c_msg_1.clone());
			queue_upward_msg(a, a_msg_1.clone());
			queue_upward_msg(a, a_msg_2.clone());

			assert_storage_consistency_exhaustive();

			// we expect only two first messages to fit in the first iteration.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(a, a_msg_1), (c, c_msg_1)]);
			assert_storage_consistency_exhaustive();

			queue_upward_msg(c, c_msg_2.clone());
			assert_storage_consistency_exhaustive();

			// second iteration should process the second message.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(q, q_msg)]);
			assert_storage_consistency_exhaustive();

			// 3rd iteration.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(a, a_msg_2), (c, c_msg_2)]);
			assert_storage_consistency_exhaustive();

			// finally, make sure that the queue is empty.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![]);
			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn dispatch_keeps_message_after_weight_exhausted() {
		let a = ParaId::from(128);

		let a_msg_1 = (300u32, "a_msg_1").encode();
		let a_msg_2 = (300u32, "a_msg_2").encode();

		new_test_ext(
			GenesisConfigBuilder {
				ump_service_total_weight: 500,
				ump_max_individual_weight: 300,
				..Default::default()
			}
			.build(),
		)
		.execute_with(|| {
			queue_upward_msg(a, a_msg_1.clone());
			queue_upward_msg(a, a_msg_2.clone());

			assert_storage_consistency_exhaustive();

			// we expect only one message to fit in the first iteration.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(a, a_msg_1)]);
			assert_storage_consistency_exhaustive();

			// second iteration should process the remaining message.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(a, a_msg_2)]);
			assert_storage_consistency_exhaustive();

			// finally, make sure that the queue is empty.
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![]);
			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn dispatch_correctly_handle_remove_of_latest() {
		let a = ParaId::from(1991);
		let b = ParaId::from(1999);

		let a_msg_1 = (300u32, "a_msg_1").encode();
		let a_msg_2 = (300u32, "a_msg_2").encode();
		let b_msg_1 = (300u32, "b_msg_1").encode();

		new_test_ext(
			GenesisConfigBuilder { ump_service_total_weight: 900, ..Default::default() }.build(),
		)
		.execute_with(|| {
			// We want to test here an edge case, where we remove the queue with the highest
			// para id (i.e. last in the needs_dispatch order).
			//
			// If the last entry was removed we should proceed execution, assuming we still have
			// weight available.

			queue_upward_msg(a, a_msg_1.clone());
			queue_upward_msg(a, a_msg_2.clone());
			queue_upward_msg(b, b_msg_1.clone());
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(a, a_msg_1), (b, b_msg_1), (a, a_msg_2)]);
		});
	}

	#[test]
	fn verify_relay_dispatch_queue_size_is_externally_accessible() {
		// Make sure that the relay dispatch queue size storage entry is accessible via well known
		// keys and is decodable into a (u32, u32).

		use parity_scale_codec::Decode as _;
		use primitives::v1::well_known_keys;

		let a = ParaId::from(228);
		let msg = vec![1, 2, 3];

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			queue_upward_msg(a, msg);

			let raw_queue_size =
				sp_io::storage::get(&well_known_keys::relay_dispatch_queue_size(a)).expect(
					"enqueing a message should create the dispatch queue\
				and it should be accessible via the well known keys",
				);
			let (cnt, size) = <(u32, u32)>::decode(&mut &raw_queue_size[..])
				.expect("the dispatch queue size should be decodable into (u32, u32)");

			assert_eq!(cnt, 1);
			assert_eq!(size, 3);
		});
	}

	#[test]
	fn service_overweight_unknown() {
		// This test just makes sure that 0 is not a valid index and we can use it not worrying in
		// the next test.
		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			assert_noop!(
				Ump::service_overweight(Origin::root(), 0, 1000),
				Error::<Test>::UnknownMessageIndex
			);
		});
	}

	#[test]
	fn overweight_queue_works() {
		let para_a = ParaId::from(2021);

		let a_msg_1 = (301u32, "a_msg_1").encode();
		let a_msg_2 = (500u32, "a_msg_2").encode();
		let a_msg_3 = (500u32, "a_msg_3").encode();

		new_test_ext(
			GenesisConfigBuilder {
				ump_service_total_weight: 900,
				ump_max_individual_weight: 300,
				..Default::default()
			}
			.build(),
		)
		.execute_with(|| {
			// HACK: Start with the block number 1. This is needed because should an event be
			// emitted during the genesis block they will be implicitly wiped.
			System::set_block_number(1);

			// This one is overweight. However, the weight is plenty and we can afford to execute
			// this message, thus expect it.
			queue_upward_msg(para_a, a_msg_1.clone());
			Ump::process_pending_upward_messages();
			assert_eq!(take_processed(), vec![(para_a, a_msg_1)]);

			// This is overweight and this message cannot fit into the total weight budget.
			queue_upward_msg(para_a, a_msg_2.clone());
			queue_upward_msg(para_a, a_msg_3.clone());
			Ump::process_pending_upward_messages();
			assert_last_event(
				Event::OverweightEnqueued(para_a, upward_message_id(&a_msg_3[..]), 0, 500).into(),
			);

			// Now verify that if we wanted to service this overweight message with less than enough
			// weight it will fail.
			assert_noop!(
				Ump::service_overweight(Origin::root(), 0, 499),
				Error::<Test>::WeightOverLimit
			);

			// ... and if we try to service it with just enough weight it will succeed as well.
			assert_ok!(Ump::service_overweight(Origin::root(), 0, 500));
			assert_last_event(Event::OverweightServiced(0, 500).into());

			// ... and if we try to service a message with index that doesn't exist it will error
			// out.
			assert_noop!(
				Ump::service_overweight(Origin::root(), 1, 1000),
				Error::<Test>::UnknownMessageIndex
			);
		});
	}
}
