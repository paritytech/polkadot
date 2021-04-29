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
use sp_std::{prelude::*, fmt, marker::PhantomData};
use sp_std::collections::{btree_map::BTreeMap, vec_deque::VecDeque};
use frame_support::{decl_module, decl_storage, StorageMap, StorageValue, weights::Weight, traits::Get};
use primitives::v1::{Id as ParaId, UpwardMessage};

const LOG_TARGET: &str = "runtime::ump-sink";

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
	fn process_upward_message(origin: ParaId, msg: &[u8], max_weight: Weight) -> Option<Weight>;
}

/// An implementation of a sink that just swallows the message without consuming any weight. Returns
/// `Some(0)` indicating that no messages existed for it to process.
impl UmpSink for () {
	fn process_upward_message(_: ParaId, _: &[u8], _: Weight) -> Option<Weight> {
		Some(0)
	}
}

/// A specific implementation of a UmpSink where messages are in the XCM format
/// and will be forwarded to the XCM Executor.
pub struct XcmSink<XcmExecutor, Call>(PhantomData<(XcmExecutor, Call)>);

impl<XcmExecutor: xcm::v0::ExecuteXcm<Call>, Call> UmpSink for XcmSink<XcmExecutor, Call> {
	fn process_upward_message(origin: ParaId, mut msg: &[u8], max_weight: Weight) -> Option<Weight> {
		use parity_scale_codec::Decode;
		use xcm::VersionedXcm;
		use xcm::v0::{Junction, MultiLocation, Outcome, Error as XcmError};

		if let Ok(versioned_xcm_message) = VersionedXcm::decode(&mut msg) {
			match versioned_xcm_message {
				VersionedXcm::V0(xcm_message) => {
					let xcm_junction: Junction = Junction::Parachain(origin.into());
					let xcm_location: MultiLocation = xcm_junction.into();
					match XcmExecutor::execute_xcm(xcm_location, xcm_message, max_weight) {
						Outcome::Complete(w) | Outcome::Incomplete(w, _) => Some(w),
						Outcome::Error(XcmError::WeightLimitReached(..)) => None,
						Outcome::Error(_) => Some(0),
					}
				}
			}
		} else {
			log::error!(
				target: LOG_TARGET,
				"Failed to decode versioned XCM from upward message.",
			);
			None
		}
	}
}

/// An error returned by [`check_upward_messages`] that indicates a violation of one of acceptance
/// criteria rules.
pub enum AcceptanceCheckErr {
	MoreMessagesThanPermitted {
		sent: u32,
		permitted: u32,
	},
	MessageSize {
		idx: u32,
		msg_size: u32,
		max_size: u32,
	},
	CapacityExceeded {
		count: u32,
		limit: u32,
	},
	TotalSizeExceeded {
		total_size: u32,
		limit: u32,
	},
}

impl fmt::Debug for AcceptanceCheckErr {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			AcceptanceCheckErr::MoreMessagesThanPermitted { sent, permitted } => write!(
				fmt,
				"more upward messages than permitted by config ({} > {})",
				sent, permitted,
			),
			AcceptanceCheckErr::MessageSize {
				idx,
				msg_size,
				max_size,
			} => write!(
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

pub trait Config: frame_system::Config + configuration::Config {
	/// A place where all received upward messages are funneled.
	type UmpSink: UmpSink;

	/// The factor by which the weight limit it multiplied for the first UMP message to execute with.
	type FirstMessageFactorPercent: Get<Weight>;
}

decl_storage! {
	trait Store for Module<T: Config> as Ump {
		/// The messages waiting to be handled by the relay-chain originating from a certain parachain.
		///
		/// Note that some upward messages might have been already processed by the inclusion logic. E.g.
		/// channel management messages.
		///
		/// The messages are processed in FIFO order.
		RelayDispatchQueues: map hasher(twox_64_concat) ParaId => VecDeque<UpwardMessage>;
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
		// NOTE that this field is used by parachains via merkle storage proofs, therefore changing
		// the format will require migration of parachains.
		RelayDispatchQueueSize: map hasher(twox_64_concat) ParaId => (u32, u32);
		/// The ordered list of `ParaId`s that have a `RelayDispatchQueue` entry.
		///
		/// Invariant:
		/// - The set of items from this vector should be exactly the set of the keys in
		///   `RelayDispatchQueues` and `RelayDispatchQueueSize`.
		NeedsDispatch: Vec<ParaId>;
		/// This is the para that gets will get dispatched first during the next upward dispatchable queue
		/// execution round.
		///
		/// Invariant:
		/// - If `Some(para)`, then `para` must be present in `NeedsDispatch`.
		NextDispatchRoundStartWith: Option<ParaId>;
	}
}

decl_module! {
	/// The UMP module.
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
	}
}

/// Routines related to the upward message passing.
impl<T: Config> Module<T> {
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
			});
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
				});
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
			});
		}
		if para_queue_size > config.max_upward_queue_size {
			return Err(AcceptanceCheckErr::TotalSizeExceeded {
				total_size: para_queue_size,
				limit: config.max_upward_queue_size,
			});
		}

		Ok(())
	}

	/// Enacts all the upward messages sent by a candidate.
	pub(crate) fn enact_upward_messages(
		para: ParaId,
		upward_messages: Vec<UpwardMessage>,
	) -> Weight {
		let mut weight = 0;

		if !upward_messages.is_empty() {
			let (extra_cnt, extra_size) = upward_messages
				.iter()
				.fold((0, 0), |(cnt, size), d| (cnt + 1, size + d.len() as u32));

			<Self as Store>::RelayDispatchQueues::mutate(&para, |v| {
				v.extend(upward_messages.into_iter())
			});

			<Self as Store>::RelayDispatchQueueSize::mutate(&para, |(ref mut cnt, ref mut size)| {
				*cnt += extra_cnt;
				*size += extra_size;
			});

			<Self as Store>::NeedsDispatch::mutate(|v| {
				if let Err(i) = v.binary_search(&para) {
					v.insert(i, para);
				}
			});

			weight += T::DbWeight::get().reads_writes(3, 3);
		}

		weight
	}

	/// Devote some time into dispatching pending upward messages.
	pub(crate) fn process_pending_upward_messages() {
		let mut used_weight_so_far = 0;

		let config = <configuration::Module<T>>::config();
		let mut cursor = NeedsDispatchCursor::new::<T>();
		let mut queue_cache = QueueCache::new();

		while let Some(dispatchee) = cursor.peek() {
			if used_weight_so_far >= config.preferred_dispatchable_upward_messages_step_weight {
				// Then check whether we've reached or overshoot the
				// preferred weight for the dispatching stage.
				//
				// if so - bail.
				break;
			}
			let max_weight = if used_weight_so_far == 0 {
				// we increase the amount of weight that we're allowed to use on the first message to try to prevent
				// the possibility of blockage of the queue.
				config.preferred_dispatchable_upward_messages_step_weight * T::FirstMessageFactorPercent::get() / 100
			} else {
				config.preferred_dispatchable_upward_messages_step_weight - used_weight_so_far
			};

			// dequeue the next message from the queue of the dispatchee
			let (upward_message, became_empty) = queue_cache.dequeue::<T>(dispatchee);
			if let Some(upward_message) = upward_message {
				match T::UmpSink::process_upward_message(dispatchee, &upward_message[..], max_weight) {
					None => break,
					Some(used) => used_weight_so_far += used,
				}
			}

			if became_empty {
				// the queue is empty now - this para doesn't need attention anymore.
				cursor.remove();
			} else {
				cursor.advance();
			}
		}

		cursor.flush::<T>();
		queue_cache.flush::<T>();
	}
}

/// To avoid constant fetching, deserializing and serialization the queues are cached.
///
/// After an item dequeued from a queue for the first time, the queue is stored in this struct rather
/// than being serialized and persisted.
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
/// On the other hand, the situation when deep queues exist and it takes more than one dipsatcher
/// cycle to traverse the queues is already sub-optimal and better be avoided.
///
/// This struct is not supposed to be dropped but rather to be consumed by [`flush`].
struct QueueCache(BTreeMap<ParaId, QueueCacheEntry>);

struct QueueCacheEntry {
	queue: VecDeque<UpwardMessage>,
	count: u32,
	total_size: u32,
}

impl QueueCache {
	fn new() -> Self {
		Self(BTreeMap::new())
	}

	/// Dequeues one item from the upward message queue of the given para.
	///
	/// Returns `(upward_message, became_empty)`, where
	///
	/// - `upward_message` a dequeued message or `None` if the queue _was_ empty.
	/// - `became_empty` is true if the queue _became_ empty.
	fn dequeue<T: Config>(&mut self, para: ParaId) -> (Option<UpwardMessage>, bool) {
		let cache_entry = self.0.entry(para).or_insert_with(|| {
			let queue = <Module<T> as Store>::RelayDispatchQueues::get(&para);
			let (count, total_size) = <Module<T> as Store>::RelayDispatchQueueSize::get(&para);
			QueueCacheEntry {
				queue,
				count,
				total_size,
			}
		});
		let upward_message = cache_entry.queue.pop_front();
		if let Some(ref msg) = upward_message {
			cache_entry.count -= 1;
			cache_entry.total_size -= msg.len() as u32;
		}

		let became_empty = cache_entry.queue.is_empty();
		(upward_message, became_empty)
	}

	/// Flushes the updated queues into the storage.
	fn flush<T: Config>(self) {
		// NOTE we use an explicit method here instead of Drop impl because it has unwanted semantics
		// within runtime. It is dangerous to use because of double-panics and flushing on a panic
		// is not necessary as well.
		for (
			para,
			QueueCacheEntry {
				queue,
				count,
				total_size,
			},
		) in self.0
		{
			if queue.is_empty() {
				// remove the entries altogether.
				<Module<T> as Store>::RelayDispatchQueues::remove(&para);
				<Module<T> as Store>::RelayDispatchQueueSize::remove(&para);
			} else {
				<Module<T> as Store>::RelayDispatchQueues::insert(&para, queue);
				<Module<T> as Store>::RelayDispatchQueueSize::insert(&para, (count, total_size));
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
	cur_idx: usize,
}

impl NeedsDispatchCursor {
	fn new<T: Config>() -> Self {
		let needs_dispatch: Vec<ParaId> = <Module<T> as Store>::NeedsDispatch::get();
		let start_with = <Module<T> as Store>::NextDispatchRoundStartWith::get();

		let start_with_idx = match start_with {
			Some(para) => match needs_dispatch.binary_search(&para) {
				Ok(found_idx) => found_idx,
				Err(_supposed_idx) => {
					// well that's weird because we maintain an invariant that
					// `NextDispatchRoundStartWith` must point into one of the items in
					// `NeedsDispatch`.
					//
					// let's select 0 as the starting index as a safe bet.
					debug_assert!(false);
					0
				}
			},
			None => 0,
		};

		Self {
			needs_dispatch,
			cur_idx: start_with_idx,
		}
	}

	/// Returns the item the cursor points to.
	fn peek(&self) -> Option<ParaId> {
		self.needs_dispatch.get(self.cur_idx).cloned()
	}

	/// Moves the cursor to the next item.
	fn advance(&mut self) {
		if self.needs_dispatch.is_empty() {
			return;
		}
		self.cur_idx = (self.cur_idx + 1) % self.needs_dispatch.len();
	}

	/// Removes the item under the cursor.
	fn remove(&mut self) {
		if self.needs_dispatch.is_empty() {
			return;
		}
		let _ = self.needs_dispatch.remove(self.cur_idx);

		// we might've removed the last element and that doesn't necessarily mean that `needs_dispatch`
		// became empty. Reposition the cursor in this case to the beginning.
		if self.needs_dispatch.get(self.cur_idx).is_none() {
			self.cur_idx = 0;
		}
	}

	/// Flushes the dispatcher state into the persistent storage.
	fn flush<T: Config>(self) {
		let next_one = self.peek();
		<Module<T> as Store>::NextDispatchRoundStartWith::set(next_one);
		<Module<T> as Store>::NeedsDispatch::put(self.needs_dispatch);
	}
}

#[cfg(test)]
pub(crate) mod mock_sink {
	//! An implementation of a mock UMP sink that allows attaching a probe for mocking the weights
	//! and checking the sent messages.
	//!
	//! A default behavior of the UMP sink is to ignore an incoming message and return 0 weight.
	//!
	//! A probe can be attached to the mock UMP sink. When attached, the mock sink would consult the
	//! probe to check whether the received message was expected and what weight it should return.
	//!
	//! There are two rules on how to use a probe:
	//!
	//! 1. There can be only one active probe at a time. Creation of another probe while there is
	//!    already an active one leads to a panic. The probe is scoped to a thread where it was created.
	//!
	//! 2. All messages expected by the probe must be received by the time of dropping it. Unreceived
	//!    messages will lead to a panic while dropping a probe.

	use super::{UmpSink, UpwardMessage, ParaId};
	use std::cell::RefCell;
	use std::collections::vec_deque::VecDeque;
	use frame_support::weights::Weight;

	#[derive(Debug)]
	struct UmpExpectation {
		expected_origin: ParaId,
		expected_msg: UpwardMessage,
		mock_weight: Weight,
	}

	std::thread_local! {
		// `Some` here indicates that there is an active probe.
		static HOOK: RefCell<Option<VecDeque<UmpExpectation>>> = RefCell::new(None);
	}

	pub struct MockUmpSink;
	impl UmpSink for MockUmpSink {
		fn process_upward_message(actual_origin: ParaId, actual_msg: &[u8], _max_weight: Weight) -> Option<Weight> {
			HOOK.with(|opt_hook| opt_hook.borrow_mut().as_mut().map(|hook| {
				let UmpExpectation {
					expected_origin,
					expected_msg,
					mock_weight,
				} = match hook.pop_front() {
					Some(expectation) => expectation,
					None => {
						panic!(
							"The probe is active but didn't expect the message:\n\n\t{:?}.",
							actual_msg,
						);
					}
				};
				assert_eq!(expected_origin, actual_origin);
				assert_eq!(expected_msg, &actual_msg[..]);
				mock_weight
			}))
		}
	}

	pub struct Probe {
		_private: (),
	}

	impl Probe {
		pub fn new() -> Self {
			HOOK.with(|opt_hook| {
				let prev = opt_hook.borrow_mut().replace(VecDeque::default());

				// that can trigger if there were two probes were created during one session which
				// is may be a bit strict, but may save time figuring out what's wrong.
				// if you land here and you do need the two probes in one session consider
				// dropping the the existing probe explicitly.
				assert!(prev.is_none());
			});
			Self { _private: () }
		}

		/// Add an expected message.
		///
		/// The enqueued messages are processed in FIFO order.
		pub fn assert_msg(
			&mut self,
			expected_origin: ParaId,
			expected_msg: UpwardMessage,
			mock_weight: Weight,
		) {
			HOOK.with(|opt_hook| {
				opt_hook
					.borrow_mut()
					.as_mut()
					.unwrap()
					.push_back(UmpExpectation {
						expected_origin,
						expected_msg,
						mock_weight,
					})
			});
		}
	}

	impl Drop for Probe {
		fn drop(&mut self) {
			let _ = HOOK.try_with(|opt_hook| {
				let prev = opt_hook.borrow_mut().take().expect(
					"this probe was created and hasn't been yet destroyed;
					the probe cannot be replaced;
					there is only one probe at a time allowed;
					thus it cannot be `None`;
					qed",
				);

				if !prev.is_empty() {
					// some messages are left unchecked. We should notify the developer about this.
					// however, we do so only if the thread doesn't panic already. Otherwise, the
					// developer would get a SIGILL or SIGABRT without a meaningful error message.
					if !std::thread::panicking() {
						panic!(
							"the probe is dropped and not all expected messages arrived: {:?}",
							prev
						);
					}
				}
			});
			// an `Err` here signals here that the thread local was already destroyed.
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use super::mock_sink::Probe;
	use crate::mock::{Configuration, Ump, new_test_ext, MockGenesisConfig};
	use frame_support::IterableStorageMap;
	use std::collections::HashSet;

	struct GenesisConfigBuilder {
		max_upward_message_size: u32,
		max_upward_message_num_per_candidate: u32,
		max_upward_queue_count: u32,
		max_upward_queue_size: u32,
		preferred_dispatchable_upward_messages_step_weight: Weight,
	}

	impl Default for GenesisConfigBuilder {
		fn default() -> Self {
			Self {
				max_upward_message_size: 16,
				max_upward_message_num_per_candidate: 2,
				max_upward_queue_count: 4,
				max_upward_queue_size: 64,
				preferred_dispatchable_upward_messages_step_weight: 1000,
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
			config.preferred_dispatchable_upward_messages_step_weight =
				self.preferred_dispatchable_upward_messages_step_weight;
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
		let _ = Ump::enact_upward_messages(para, msgs);
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
		let needs_dispatch_set = <Ump as Store>::NeedsDispatch::get()
			.into_iter()
			.collect::<HashSet<ParaId>>();
		assert_eq!(queue_contents_set, queue_sizes_set);
		assert_eq!(queue_contents_set, needs_dispatch_set);

		// `NextDispatchRoundStartWith` should point into a para that is tracked.
		if let Some(para) = <Ump as Store>::NextDispatchRoundStartWith::get() {
			assert!(queue_contents_set.contains(&para));
		}

		// `NeedsDispatch` is always sorted.
		assert!(
			<Ump as Store>::NeedsDispatch::get()
				.windows(2)
				.all(|xs| xs[0] <= xs[1])
		);
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
		let msg = vec![1, 2, 3];

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			let mut probe = Probe::new();

			probe.assert_msg(a, msg.clone(), 0);
			queue_upward_msg(a, msg);

			Ump::process_pending_upward_messages();

			assert_storage_consistency_exhaustive();
		});
	}

	#[test]
	fn dispatch_resume_after_exceeding_dispatch_stage_weight() {
		let a = ParaId::from(128);
		let c = ParaId::from(228);
		let q = ParaId::from(911);

		let a_msg_1 = vec![1, 2, 3];
		let a_msg_2 = vec![3, 2, 1];
		let c_msg_1 = vec![4, 5, 6];
		let c_msg_2 = vec![9, 8, 7];
		let q_msg = b"we are Q".to_vec();

		new_test_ext(
			GenesisConfigBuilder {
				preferred_dispatchable_upward_messages_step_weight: 500,
				..Default::default()
			}
			.build(),
		)
		.execute_with(|| {
			queue_upward_msg(q, q_msg.clone());
			queue_upward_msg(c, c_msg_1.clone());
			queue_upward_msg(a, a_msg_1.clone());
			queue_upward_msg(a, a_msg_2.clone());

			assert_storage_consistency_exhaustive();

			// we expect only two first messages to fit in the first iteration.
			{
				let mut probe = Probe::new();

				probe.assert_msg(a, a_msg_1.clone(), 300);
				probe.assert_msg(c, c_msg_1.clone(), 300);
				Ump::process_pending_upward_messages();
				assert_storage_consistency_exhaustive();

				drop(probe);
			}

			queue_upward_msg(c, c_msg_2.clone());
			assert_storage_consistency_exhaustive();

			// second iteration should process the second message.
			{
				let mut probe = Probe::new();

				probe.assert_msg(q, q_msg.clone(), 500);
				Ump::process_pending_upward_messages();
				assert_storage_consistency_exhaustive();

				drop(probe);
			}

			// 3rd iteration.
			{
				let mut probe = Probe::new();

				probe.assert_msg(a, a_msg_2.clone(), 100);
				probe.assert_msg(c, c_msg_2.clone(), 100);
				Ump::process_pending_upward_messages();
				assert_storage_consistency_exhaustive();

				drop(probe);
			}

			// finally, make sure that the queue is empty.
			{
				let probe = Probe::new();

				Ump::process_pending_upward_messages();
				assert_storage_consistency_exhaustive();

				drop(probe);
			}
		});
	}

	#[test]
	fn dispatch_correctly_handle_remove_of_latest() {
		let a = ParaId::from(1991);
		let b = ParaId::from(1999);

		let a_msg_1 = vec![1, 2, 3];
		let a_msg_2 = vec![3, 2, 1];
		let b_msg_1 = vec![4, 5, 6];

		new_test_ext(
			GenesisConfigBuilder {
				preferred_dispatchable_upward_messages_step_weight: 900,
				..Default::default()
			}
			.build(),
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

			{
				let mut probe = Probe::new();

				probe.assert_msg(a, a_msg_1.clone(), 300);
				probe.assert_msg(b, b_msg_1.clone(), 300);
				probe.assert_msg(a, a_msg_2.clone(), 300);

				Ump::process_pending_upward_messages();

				drop(probe);
			}
		});
	}

	#[test]
	fn verify_relay_dispatch_queue_size_is_externally_accessible() {
		// Make sure that the relay dispatch queue size storage entry is accessible via well known
		// keys and is decodable into a (u32, u32).

		use primitives::v1::well_known_keys;
		use parity_scale_codec::Decode as _;

		let a = ParaId::from(228);
		let msg = vec![1, 2, 3];

		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			queue_upward_msg(a, msg);

			let raw_queue_size = sp_io::storage::get(&well_known_keys::relay_dispatch_queue_size(a))
				.expect("enqueing a message should create the dispatch queue\
				and it should be accessible via the well known keys");
			let (cnt, size) = <(u32, u32)>::decode(&mut &raw_queue_size[..])
				.expect("the dispatch queue size should be decodable into (u32, u32)");

			assert_eq!(cnt, 1);
			assert_eq!(size, 3);
		});
	}
}
