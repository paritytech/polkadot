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

//! The scheduler module for parachains and parathreads.
//!
//! This module is responsible for two main tasks:
//!   - Paritioning validators into groups and assigning groups to parachains and parathreads
//!   - Scheduling parachains and parathreads
//!
//! It aims to achieve these tasks with these goals in mind:
//! - It should be possible to know at least a block ahead-of-time, ideally more,
//!   which validators are going to be assigned to which parachains.
//! - Parachains that have a candidate pending availability in this fork of the chain
//!   should not be assigned.
//! - Validator assignments should not be gameable. Malicious cartels should not be able to
//!   manipulate the scheduler to assign themselves as desired.
//! - High or close to optimal throughput of parachains and parathreads. Work among validator groups should be balanced.
//!
//! The Scheduler manages resource allocation using the concept of "Availability Cores".
//! There will be one availability core for each parachain, and a fixed number of cores
//! used for multiplexing parathreads. Validators will be partitioned into groups, with the same
//! number of groups as availability cores. Validator groups will be assigned to different availability cores
//! over time.

use sp_std::prelude::*;
use sp_std::convert::TryInto;
use primitives::{
	parachain::{
		Id as ParaId, ValidatorIndex, CoreAssignment, CoreOccupied, CoreIndex, AssignmentKind,
		GroupIndex, ParathreadClaim, ParathreadEntry,
	},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
	weights::Weight,
};
use codec::{Encode, Decode};
use sp_runtime::traits::{Saturating, Zero};

use rand::{SeedableRng, seq::SliceRandom};
use rand_chacha::ChaCha20Rng;

use crate::{configuration, paras, initializer::SessionChangeNotification};

/// A queued parathread entry, pre-assigned to a core.
#[derive(Encode, Decode, Default)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct QueuedParathread {
	claim: ParathreadEntry,
	core_offset: u32,
}

/// The queue of all parathread claims.
#[derive(Encode, Decode, Default)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct ParathreadClaimQueue {
	queue: Vec<QueuedParathread>,
	// this value is between 0 and config.parathread_cores
	next_core_offset: u32,
}

impl ParathreadClaimQueue {
	/// Queue a parathread entry to be processed.
	///
	/// Provide the entry and the number of parathread cores, which must be greater than 0.
	fn enqueue_entry(&mut self, entry: ParathreadEntry, n_parathread_cores: u32) {
		let core_offset = self.next_core_offset;
		self.next_core_offset = (self.next_core_offset + 1) % n_parathread_cores;

		self.queue.push(QueuedParathread {
			claim: entry,
			core_offset,
		})
	}

	// Take next queued entry with given core offset, if any.
	fn take_next_on_core(&mut self, core_offset: u32) -> Option<ParathreadEntry> {
		let pos = self.queue.iter().position(|queued| queued.core_offset == core_offset);
		pos.map(|i| self.queue.remove(i).claim)
	}
}

/// Reasons a core might be freed
pub enum FreedReason {
	/// The core's work concluded and the parablock assigned to it is considered available.
	Concluded,
	/// The core's work timed out.
	TimedOut,
}

pub trait Trait: system::Trait + configuration::Trait + paras::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as ParaScheduler {
		/// All the validator groups. One for each core.
		///
		/// Bound: The number of cores is the sum of the numbers of parachains and parathread multiplexers.
		/// Reasonably, 100-1000. The dominant factor is the number of validators: safe upper bound at 10k.
		ValidatorGroups: Vec<Vec<ValidatorIndex>>;

		/// A queue of upcoming claims and which core they should be mapped onto.
		///
		/// The number of queued claims is bounded at the `scheduling_lookahead`
		/// multiplied by the number of parathread multiplexer cores. Reasonably, 10 * 50 = 500.
		ParathreadQueue: ParathreadClaimQueue;
		/// One entry for each availability core. Entries are `None` if the core is not currently occupied. Can be
		/// temporarily `Some` if scheduled but not occupied.
		/// The i'th parachain belongs to the i'th core, with the remaining cores all being
		/// parathread-multiplexers.
		///
		/// Bounded by the number of cores: one for each parachain and parathread multiplexer.
		AvailabilityCores: Vec<Option<CoreOccupied>>;
		/// An index used to ensure that only one claim on a parathread exists in the queue or is
		/// currently being handled by an occupied core.
		///
		/// Bounded by the number of parathread cores and scheduling lookahead. Reasonably, 10 * 50 = 500.
		ParathreadClaimIndex: Vec<ParaId>;
		/// The block number where the session start occurred. Used to track how many group rotations have occurred.
		SessionStartBlock: T::BlockNumber;
		/// Currently scheduled cores - free but up to be occupied. Ephemeral storage item that's wiped on finalization.
		///
		/// Bounded by the number of cores: one for each parachain and parathread multiplexer.
		Scheduled get(fn scheduled): Vec<CoreAssignment>; // sorted ascending by CoreIndex.
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The scheduler module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin, system = system {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {
	/// Called by the initializer to initialize the scheduler module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		Self::schedule(Vec::new());

		0
	}

	/// Called by the initializer to finalize the scheduler module.
	pub(crate) fn initializer_finalize() {
		// Free all scheduled cores and return parathread claims to queue, with retries incremented.
		let config = <configuration::Module<T>>::config();
		ParathreadQueue::mutate(|queue| {
			for core_assignment in Scheduled::take() {
				if let AssignmentKind::Parathread(collator, retries) = core_assignment.kind {
					let entry = ParathreadEntry {
						claim: ParathreadClaim(core_assignment.para_id, collator),
						retries: retries + 1,
					};

					if entry.retries <= config.parathread_retries {
						queue.enqueue_entry(entry, config.parathread_cores);
					}
				}
			}
		})

	}

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(notification: &SessionChangeNotification<T::BlockNumber>) {
		let &SessionChangeNotification {
			ref validators,
			ref random_seed,
			ref new_config,
			..
		} = notification;
		let config = new_config;

		let mut thread_queue = ParathreadQueue::get();
		let n_parachains = <paras::Module<T>>::parachains().len() as u32;
		let n_cores = n_parachains + config.parathread_cores;

		<SessionStartBlock<T>>::set(<system::Module<T>>::block_number());
		AvailabilityCores::mutate(|cores| {
			// clear all occupied cores.
			for maybe_occupied in cores.iter_mut() {
				if let Some(CoreOccupied::Parathread(claim)) = maybe_occupied.take() {
					let queued = QueuedParathread {
						claim,
						core_offset: 0, // this gets set later in the re-balancing.
					};

					thread_queue.queue.push(queued);
				}
			}

			cores.resize(n_cores as _, None);
		});

		// shuffle validators into groups.
		if n_cores == 0 || validators.is_empty() {
			ValidatorGroups::set(Vec::new());
		} else {
			let mut rng: ChaCha20Rng = SeedableRng::from_seed(*random_seed);

			let mut shuffled_indices: Vec<_> = (0..validators.len())
				.enumerate()
				.map(|(i, _)| i as ValidatorIndex)
				.collect();

			shuffled_indices.shuffle(&mut rng);

			let group_base_size = validators.len() / n_cores as usize;
			let n_larger_groups = validators.len() % n_cores as usize;
			let groups: Vec<Vec<_>> = (0..n_cores).map(|core_id| {
				let n_members = if (core_id as usize) < n_larger_groups {
					group_base_size + 1
				} else {
					group_base_size
				};

				shuffled_indices.drain(shuffled_indices.len() - n_members ..).rev().collect()
			}).collect();

			ValidatorGroups::set(groups);
		}

		// prune out all parathread claims with too many retries.
		// assign all non-pruned claims to new cores, if they've changed.
		ParathreadClaimIndex::mutate(|claim_index| {
			// wipe all parathread metadata if no parathread cores are configured.
			if config.parathread_cores == 0 {
				thread_queue = ParathreadClaimQueue {
					queue: Vec::new(),
					next_core_offset: 0,
				};
				claim_index.clear();
				return;
			}

			// prune out all entries beyond retry or that no longer correspond to live parathread.
			thread_queue.queue.retain(|queued| {
				let will_keep = queued.claim.retries <= config.parathread_retries
					&& <paras::Module<T>>::is_parathread(queued.claim.claim.0);

				if !will_keep {
					let claim_para = queued.claim.claim.0;

					// clean up the pruned entry from the index.
					if let Ok(i) = claim_index.binary_search(&claim_para) {
						claim_index.remove(i);
					}
				}

				will_keep
			});

			// do re-balancing of claims.
			{
				for (i, queued) in thread_queue.queue.iter_mut().enumerate() {
					queued.core_offset = (i as u32) % config.parathread_cores;
				}

				thread_queue.next_core_offset =
					((thread_queue.queue.len()) as u32) % config.parathread_cores;
			}
		});
		ParathreadQueue::set(thread_queue);
	}

	/// Add a parathread claim to the queue. If there is a competing claim in the queue or currently
	/// assigned to a core, this call will fail. This call will also fail if the queue is full.
	///
	/// Fails if the claim does not correspond to any live parathread.
	#[allow(unused)]
	pub fn add_parathread_claim(claim: ParathreadClaim) {
		if !<paras::Module<T>>::is_parathread(claim.0) { return }

		let config = <configuration::Module<T>>::config();
		let queue_max_size = config.parathread_cores * config.scheduling_lookahead;

		ParathreadQueue::mutate(|queue| {
			if queue.queue.len() >= queue_max_size as usize { return }

			let para_id = claim.0;

			let competes_with_another = ParathreadClaimIndex::mutate(|index| {
				match index.binary_search(&para_id) {
					Ok(_) => true,
					Err(i) => {
						index.insert(i, para_id);
						false
					}
				}
			});

			if competes_with_another { return }

			let entry = ParathreadEntry { claim, retries: 0 };
			queue.enqueue_entry(entry, config.parathread_cores);
		})
	}

	/// Schedule all unassigned cores, where possible. Provide a list of cores that should be considered
	/// newly-freed along with the reason for them being freed. The list is assumed to be sorted in
	/// ascending order by core index.
	pub(crate) fn schedule(just_freed_cores: Vec<(CoreIndex, FreedReason)>) {
		let mut cores = AvailabilityCores::get();
		let config = <configuration::Module<T>>::config();

		for (freed_index, freed_reason) in just_freed_cores {
			if (freed_index.0 as usize) < cores.len() {
				match cores[freed_index.0 as usize].take() {
					None => continue,
					Some(CoreOccupied::Parachain) => {},
					Some(CoreOccupied::Parathread(entry)) => {
						match freed_reason {
							FreedReason::Concluded => {
								// After a parathread candidate has successfully been included,
								// open it up for further claims!
								ParathreadClaimIndex::mutate(|index| {
									if let Ok(i) = index.binary_search(&entry.claim.0) {
										index.remove(i);
									}
								})
							}
							FreedReason::TimedOut => {
								// If a parathread candidate times out, it's not the collator's fault,
								// so we don't increment retries.
								ParathreadQueue::mutate(|queue| {
									queue.enqueue_entry(entry, config.parathread_cores);
								})
							}
						}
					}
				}
			}
		}

		let parachains = <paras::Module<T>>::parachains();
		let mut scheduled = Scheduled::get();
		let mut parathread_queue = ParathreadQueue::get();
		let now = <system::Module<T>>::block_number();

		if ValidatorGroups::get().is_empty() { return }

		{
			let mut prev_scheduled_in_order = scheduled.iter().enumerate().peekable();

			// Updates to the previous list of scheduled updates and the position of where to insert
			// them, without accounting for prior updates.
			let mut scheduled_updates: Vec<(usize, CoreAssignment)> = Vec::new();

			// single-sweep O(n) in the number of cores.
			for (core_index, _core) in cores.iter().enumerate().filter(|(_, ref c)| c.is_none()) {
				let schedule_and_insert_at = {
					// advance the iterator until just before the core index we are looking at now.
					while prev_scheduled_in_order.peek().map_or(
						false,
						|(_, assign)| (assign.core.0 as usize) < core_index,
					) {
						let _ = prev_scheduled_in_order.next();
					}

					// check the first entry already scheduled with core index >= than the one we
					// are looking at. 3 cases:
					//  1. No such entry, clearly this core is not scheduled, so we need to schedule and put at the end.
					//  2. Entry exists and has same index as the core we are inspecting. do not schedule again.
					//  3. Entry exists and has higher index than the core we are inspecting. schedule and note
					//     insertion position.
					prev_scheduled_in_order.peek().map_or(
						Some(scheduled.len()),
						|(idx_in_scheduled, assign)| if (assign.core.0 as usize) == core_index {
							None
						} else {
							Some(*idx_in_scheduled)
						},
					)
				};

				let schedule_and_insert_at = match schedule_and_insert_at {
					None => continue,
					Some(at) => at,
				};

				let core = CoreIndex(core_index as u32);

				let core_assignment = if core_index < parachains.len() {
					// parachain core.
					Some(CoreAssignment {
						kind: AssignmentKind::Parachain,
						para_id: parachains[core_index],
						core: core.clone(),
						group_idx: Self::group_assigned_to_core(core, now)
							.expect("core is not out of bounds and we are guaranteed \
									to be after the most recent session start; qed"),
					})
				} else {
					// parathread core offset, rel. to beginning.
					let core_offset = (core_index - parachains.len()) as u32;

					parathread_queue.take_next_on_core(core_offset).map(|entry| CoreAssignment {
						kind: AssignmentKind::Parathread(entry.claim.1, entry.retries),
						para_id: entry.claim.0,
						core: core.clone(),
						group_idx: Self::group_assigned_to_core(core, now)
							.expect("core is not out of bounds and we are guaranteed \
									to be after the most recent session start; qed"),
					})
				};

				if let Some(assignment) = core_assignment {
					scheduled_updates.push((schedule_and_insert_at, assignment))
				}
			}

			// at this point, because `Scheduled` is guaranteed to be sorted and we navigated unassigned
			// core indices in ascending order, we can enact the updates prepared by the previous actions.
			//
			// while inserting, we have to account for the amount of insertions already done.
			//
			// This is O(n) as well, capped at n operations, where n is the number of cores.
			for (num_insertions_before, (insert_at, to_insert)) in scheduled_updates.into_iter().enumerate() {
				let insert_at = num_insertions_before + insert_at;
				scheduled.insert(insert_at, to_insert);
			}

			// scheduled is guaranteed to be sorted after this point because it was sorted before, and we
			// applied sorted updates at their correct positions, accounting for the offsets of previous
			// insertions.
		}

		Scheduled::set(scheduled);
		ParathreadQueue::set(parathread_queue);
	}

	/// Note that the given cores have become occupied. Behavior undefined if any of the given cores were not scheduled
	/// or the slice is not sorted ascending by core index.
	///
	/// Complexity: O(n) in the number of scheduled cores, which is capped at the number of total cores.
	/// This is efficient in the case that most scheduled cores are occupied.
	pub(crate) fn occupied(now_occupied: &[CoreIndex]) {
		if now_occupied.is_empty() { return }

		let mut availability_cores = AvailabilityCores::get();
		Scheduled::mutate(|scheduled| {
			// The constraints on the function require that now_occupied is a sorted subset of the
			// `scheduled` cores, which are also sorted.

			let mut occupied_iter = now_occupied.iter().cloned().peekable();
			scheduled.retain(|assignment| {
				let retain = occupied_iter.peek().map_or(true, |occupied_idx| {
					occupied_idx != &assignment.core
				});

				if !retain {
					// remove this entry - it's now occupied. and begin inspecting the next extry
					// of the occupied iterator.
					let _ = occupied_iter.next();

					availability_cores[assignment.core.0 as usize] = Some(assignment.to_core_occupied());
				}

				retain
			})
		});

		AvailabilityCores::set(availability_cores);
	}

	/// Get the para (chain or thread) ID assigned to a particular core or index, if any. Core indices
	/// out of bounds will return `None`, as will indices of unassigned cores.
	pub(crate) fn core_para(core_index: CoreIndex) -> Option<ParaId> {
		let cores = AvailabilityCores::get();
		match cores.get(core_index.0 as usize).and_then(|c| c.as_ref()) {
			None => None,
			Some(CoreOccupied::Parachain) => {
				let parachains = <paras::Module<T>>::parachains();
				Some(parachains[core_index.0 as usize])
			}
			Some(CoreOccupied::Parathread(ref entry)) => Some(entry.claim.0),
		}
	}

	/// Get the validators in the given group, if the group index is valid for this session.
	pub(crate) fn group_validators(group_index: GroupIndex) -> Option<Vec<ValidatorIndex>> {
		ValidatorGroups::get().get(group_index.0 as usize).map(|g| g.clone())
	}

	/// Get the group assigned to a specific core by index at the current block number. Result undefined if the core index is unknown
	/// or the block number is less than the session start index.
	pub(crate) fn group_assigned_to_core(core: CoreIndex, at: T::BlockNumber) -> Option<GroupIndex> {
		let config = <configuration::Module<T>>::config();
		let session_start_block = <SessionStartBlock<T>>::get();

		if at < session_start_block { return None }

		if config.parachain_rotation_frequency.is_zero() {
			// interpret this as "no rotations"
			return Some(GroupIndex(core.0));
		}

		let validator_groups = ValidatorGroups::get();

		if core.0 as usize >= validator_groups.len() { return None }

		let rotations_since_session_start: T::BlockNumber =
			(at - session_start_block) / config.parachain_rotation_frequency.into();

		let rotations_since_session_start
			= match <T::BlockNumber as TryInto<u32>>::try_into(rotations_since_session_start)
		{
			Ok(i) => i,
			Err(_) => 0, // can only happen if rotations occur only once every u32::max(),
			             // so functionally no difference in behavior.
		};

		let group_idx = (core.0 as usize + rotations_since_session_start as usize) % validator_groups.len();
		Some(GroupIndex(group_idx as u32))
	}

	/// Returns an optional predicate that should be used for timing out occupied cores.
	///
	/// If `None`, no timing-out should be done. The predicate accepts the index of the core, and the
	/// block number since which it has been occupied, and the respective parachain and parathread
	/// timeouts, i.e. only within `max(config.chain_availability_period, config.thread_availability_period)`
	/// of the last rotation would this return `Some`.
	///
	/// This really should not be a box, but is working around a compiler limitation filed here:
	/// https://github.com/rust-lang/rust/issues/73226
	/// which prevents us from testing the code if using `impl Trait`.
	pub(crate) fn availability_timeout_predicate() -> Option<Box<dyn Fn(CoreIndex, T::BlockNumber) -> bool>> {
		let now = <system::Module<T>>::block_number();
		let config = <configuration::Module<T>>::config();

		let session_start = <SessionStartBlock<T>>::get();
		let blocks_since_session_start = now.saturating_sub(session_start);
		let blocks_since_last_rotation = blocks_since_session_start % config.parachain_rotation_frequency;

		let absolute_cutoff = sp_std::cmp::max(
			config.chain_availability_period,
			config.thread_availability_period,
		);

		let availability_cores = AvailabilityCores::get();

		if blocks_since_last_rotation >= absolute_cutoff {
			None
		} else {
			Some(Box::new(move |core_index: CoreIndex, pending_since| {
				match availability_cores.get(core_index.0 as usize) {
					None => true, // out-of-bounds, doesn't really matter what is returned.
					Some(None) => true, // core not occupied, still doesn't really matter.
					Some(Some(CoreOccupied::Parachain)) => {
						if blocks_since_last_rotation >= config.chain_availability_period {
							false // no pruning except recently after rotation.
						} else {
							now.saturating_sub(pending_since) >= config.chain_availability_period
						}
					}
					Some(Some(CoreOccupied::Parathread(_))) => {
						if blocks_since_last_rotation >= config.thread_availability_period {
							false // no pruning except recently after rotation.
						} else {
							now.saturating_sub(pending_since) >= config.thread_availability_period
						}
					}
				}
			}))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use primitives::{BlockNumber, parachain::{CollatorId, ValidatorId}};
	use frame_support::traits::{OnFinalize, OnInitialize};
	use keyring::Sr25519Keyring;

	use crate::mock::{new_test_ext, Configuration, Paras, System, Scheduler, GenesisConfig as MockGenesisConfig};
	use crate::initializer::SessionChangeNotification;
	use crate::configuration::HostConfiguration;
	use crate::paras::ParaGenesisArgs;

	fn run_to_block(
		to: BlockNumber,
		new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
	) {
		while System::block_number() < to {
			let b = System::block_number();

			Scheduler::initializer_finalize();
			Paras::initializer_finalize();

			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			if let Some(notification) = new_session(b + 1) {
				Paras::initializer_on_new_session(&notification);
				Scheduler::initializer_on_new_session(&notification);
			}

			Paras::initializer_initialize(b + 1);
			Scheduler::initializer_initialize(b + 1);
		}
	}

	fn default_config() -> HostConfiguration<BlockNumber> {
		HostConfiguration {
			parathread_cores: 3,
			parachain_rotation_frequency: 10,
			chain_availability_period: 3,
			thread_availability_period: 5,
			scheduling_lookahead: 2,
			parathread_retries: 1,
			..Default::default()
		}
	}

	#[test]
	fn add_parathread_claim_works() {
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};

		let thread_id = ParaId::from(10);
		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		new_test_ext(genesis_config).execute_with(|| {
			Paras::schedule_para_initialize(thread_id, ParaGenesisArgs {
				genesis_head: Vec::new().into(),
				validation_code: Vec::new().into(),
				parachain: false,
			});

			assert!(!Paras::is_parathread(thread_id));

			run_to_block(10, |n| if n == 10 { Some(Default::default()) } else { None });

			assert!(Paras::is_parathread(thread_id));

			{
				Scheduler::add_parathread_claim(ParathreadClaim(thread_id, collator.clone()));
				let queue = ParathreadQueue::get();
				assert_eq!(queue.next_core_offset, 1);
				assert_eq!(queue.queue.len(), 1);
				assert_eq!(queue.queue[0], QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_id, collator.clone()),
						retries: 0,
					},
					core_offset: 0,
				});
			}

			// due to the index, completing claims are not allowed.
			{
				let collator2 = CollatorId::from(Sr25519Keyring::Bob.public());
				Scheduler::add_parathread_claim(ParathreadClaim(thread_id, collator2.clone()));
				let queue = ParathreadQueue::get();
				assert_eq!(queue.next_core_offset, 1);
				assert_eq!(queue.queue.len(), 1);
				assert_eq!(queue.queue[0], QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_id, collator.clone()),
						retries: 0,
					},
					core_offset: 0,
				});
			}

			// claims on non-live parathreads have no effect.
			{
				let thread_id2 = ParaId::from(11);
				Scheduler::add_parathread_claim(ParathreadClaim(thread_id2, collator.clone()));
				let queue = ParathreadQueue::get();
				assert_eq!(queue.next_core_offset, 1);
				assert_eq!(queue.queue.len(), 1);
				assert_eq!(queue.queue[0], QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_id, collator.clone()),
						retries: 0,
					},
					core_offset: 0,
				});
			}
		})
	}

	#[test]
	fn cannot_add_claim_when_no_parathread_cores() {
		let config = {
			let mut config = default_config();
			config.parathread_cores = 0;
			config
		};
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config,
				..Default::default()
			},
			..Default::default()
		};

		let thread_id = ParaId::from(10);
		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		new_test_ext(genesis_config).execute_with(|| {
			Paras::schedule_para_initialize(thread_id, ParaGenesisArgs {
				genesis_head: Vec::new().into(),
				validation_code: Vec::new().into(),
				parachain: false,
			});

			assert!(!Paras::is_parathread(thread_id));

			run_to_block(10, |n| if n == 10 { Some(Default::default()) } else { None });

			assert!(Paras::is_parathread(thread_id));

			Scheduler::add_parathread_claim(ParathreadClaim(thread_id, collator.clone()));
			assert_eq!(ParathreadQueue::get(), Default::default());
		});
	}

	#[test]
	fn session_change_prunes_cores_beyond_retries_and_those_from_non_live_parathreads() {
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};
		let max_parathread_retries = default_config().parathread_retries;

		let thread_a = ParaId::from(1);
		let thread_b = ParaId::from(2);
		let thread_c = ParaId::from(3);
		let thread_d = ParaId::from(4);

		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		new_test_ext(genesis_config).execute_with(|| {
			assert_eq!(Configuration::config(), default_config());

			// threads a, b, and c will be live in next session, but not d.
			{
				Paras::schedule_para_initialize(thread_a, ParaGenesisArgs {
					genesis_head: Vec::new().into(),
					validation_code: Vec::new().into(),
					parachain: false,
				});

				Paras::schedule_para_initialize(thread_b, ParaGenesisArgs {
					genesis_head: Vec::new().into(),
					validation_code: Vec::new().into(),
					parachain: false,
				});

				Paras::schedule_para_initialize(thread_c, ParaGenesisArgs {
					genesis_head: Vec::new().into(),
					validation_code: Vec::new().into(),
					parachain: false,
				});
			}

			// set up a queue as if n_cores was 4 and with some with many retries.
			ParathreadQueue::put({
				let mut queue = ParathreadClaimQueue::default();

				// Will be pruned: too many retries.
				queue.enqueue_entry(ParathreadEntry {
					claim: ParathreadClaim(thread_a, collator.clone()),
					retries: max_parathread_retries + 1,
				}, 4);

				// Will not be pruned.
				queue.enqueue_entry(ParathreadEntry {
					claim: ParathreadClaim(thread_b, collator.clone()),
					retries: max_parathread_retries,
				}, 4);

				// Will not be pruned.
				queue.enqueue_entry(ParathreadEntry {
					claim: ParathreadClaim(thread_c, collator.clone()),
					retries: 0,
				}, 4);

				// Will be pruned: not a live parathread.
				queue.enqueue_entry(ParathreadEntry {
					claim: ParathreadClaim(thread_d, collator.clone()),
					retries: 0,
				}, 4);

				queue
			});

			ParathreadClaimIndex::put(vec![thread_a, thread_b, thread_c, thread_d]);

			run_to_block(
				10,
				|b| match b {
					10 => Some(SessionChangeNotification {
						new_config: Configuration::config(),
						..Default::default()
					}),
					_ => None,
				}
			);
			assert_eq!(Configuration::config(), default_config());

			let queue = ParathreadQueue::get();
			assert_eq!(
				queue.queue,
				vec![
					QueuedParathread {
						claim: ParathreadEntry {
							claim: ParathreadClaim(thread_b, collator.clone()),
							retries: max_parathread_retries,
						},
						core_offset: 0,
					},
					QueuedParathread {
						claim: ParathreadEntry {
							claim: ParathreadClaim(thread_c, collator.clone()),
							retries: 0,
						},
						core_offset: 1,
					},
				]
			);
			assert_eq!(queue.next_core_offset, 2);

			assert_eq!(ParathreadClaimIndex::get(), vec![thread_b, thread_c]);
		})
	}

	#[test]
	fn session_change_shuffles_validators() {
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};

		assert_eq!(default_config().parathread_cores, 3);
		new_test_ext(genesis_config).execute_with(|| {
			let chain_a = ParaId::from(1);
			let chain_b = ParaId::from(2);

			// ensure that we have 5 groups by registering 2 parachains.
			Paras::schedule_para_initialize(chain_a, ParaGenesisArgs {
				genesis_head: Vec::new().into(),
				validation_code: Vec::new().into(),
				parachain: true,
			});
			Paras::schedule_para_initialize(chain_b, ParaGenesisArgs {
				genesis_head: Vec::new().into(),
				validation_code: Vec::new().into(),
				parachain: true,
			});

			run_to_block(1, |number| match number {
				1 => Some(SessionChangeNotification {
					new_config: default_config(),
					validators: vec![
						ValidatorId::from(Sr25519Keyring::Alice.public()),
						ValidatorId::from(Sr25519Keyring::Bob.public()),
						ValidatorId::from(Sr25519Keyring::Charlie.public()),
						ValidatorId::from(Sr25519Keyring::Dave.public()),
						ValidatorId::from(Sr25519Keyring::Eve.public()),
						ValidatorId::from(Sr25519Keyring::Ferdie.public()),
						ValidatorId::from(Sr25519Keyring::One.public()),
					],
					random_seed: [99; 32],
					..Default::default()
				}),
				_ => None,
			});

			let groups = ValidatorGroups::get();
			assert_eq!(groups.len(), 5);

			// first two groups have the overflow.
			for i in 0..2 {
				assert_eq!(groups[i].len(), 2);
			}

			for i in 2..5 {
				assert_eq!(groups[i].len(), 1);
			}
		});
	}

	#[test]
	fn schedule_schedules() {
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};

		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let thread_a = ParaId::from(3);
		let thread_b = ParaId::from(4);
		let thread_c = ParaId::from(5);

		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		let schedule_blank_para = |id, is_chain| Paras::schedule_para_initialize(id, ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: Vec::new().into(),
			parachain: is_chain,
		});

		new_test_ext(genesis_config).execute_with(|| {
			assert_eq!(default_config().parathread_cores, 3);

			// register 2 parachains
			schedule_blank_para(chain_a, true);
			schedule_blank_para(chain_b, true);

			// and 3 parathreads
			schedule_blank_para(thread_a, false);
			schedule_blank_para(thread_b, false);
			schedule_blank_para(thread_c, false);

			// start a new session to activate, 5 validators for 5 cores.
			run_to_block(1, |number| match number {
				1 => Some(SessionChangeNotification {
					new_config: default_config(),
					validators: vec![
						ValidatorId::from(Sr25519Keyring::Alice.public()),
						ValidatorId::from(Sr25519Keyring::Bob.public()),
						ValidatorId::from(Sr25519Keyring::Charlie.public()),
						ValidatorId::from(Sr25519Keyring::Dave.public()),
						ValidatorId::from(Sr25519Keyring::Eve.public()),
					],
					..Default::default()
				}),
				_ => None,
			});

			{
				let scheduled = Scheduler::scheduled();
				assert_eq!(scheduled.len(), 2);

				assert_eq!(scheduled[0], CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				});

				assert_eq!(scheduled[1], CoreAssignment {
					core: CoreIndex(1),
					para_id: chain_b,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(1),
				});
			}

			// add a couple of parathread claims.
			Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
			Scheduler::add_parathread_claim(ParathreadClaim(thread_c, collator.clone()));

			run_to_block(2, |_| None);

			{
				let scheduled = Scheduler::scheduled();
				assert_eq!(scheduled.len(), 4);

				assert_eq!(scheduled[0], CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				});

				assert_eq!(scheduled[1], CoreAssignment {
					core: CoreIndex(1),
					para_id: chain_b,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(1),
				});

				assert_eq!(scheduled[2], CoreAssignment{
					core: CoreIndex(2),
					para_id: thread_a,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(2),
				});

				assert_eq!(scheduled[3], CoreAssignment{
					core: CoreIndex(3),
					para_id: thread_c,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(3),
				});
			}
		});
	}

	#[test]
	fn schedule_schedules_including_just_freed() {
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};

		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let thread_a = ParaId::from(3);
		let thread_b = ParaId::from(4);
		let thread_c = ParaId::from(5);
		let thread_d = ParaId::from(6);
		let thread_e = ParaId::from(7);

		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		let schedule_blank_para = |id, is_chain| Paras::schedule_para_initialize(id, ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: Vec::new().into(),
			parachain: is_chain,
		});

		new_test_ext(genesis_config).execute_with(|| {
			assert_eq!(default_config().parathread_cores, 3);

			// register 2 parachains
			schedule_blank_para(chain_a, true);
			schedule_blank_para(chain_b, true);

			// and 5 parathreads
			schedule_blank_para(thread_a, false);
			schedule_blank_para(thread_b, false);
			schedule_blank_para(thread_c, false);
			schedule_blank_para(thread_d, false);
			schedule_blank_para(thread_e, false);

			// start a new session to activate, 5 validators for 5 cores.
			run_to_block(1, |number| match number {
				1 => Some(SessionChangeNotification {
					new_config: default_config(),
					validators: vec![
						ValidatorId::from(Sr25519Keyring::Alice.public()),
						ValidatorId::from(Sr25519Keyring::Bob.public()),
						ValidatorId::from(Sr25519Keyring::Charlie.public()),
						ValidatorId::from(Sr25519Keyring::Dave.public()),
						ValidatorId::from(Sr25519Keyring::Eve.public()),
					],
					..Default::default()
				}),
				_ => None,
			});

			// add a couple of parathread claims now that the parathreads are live.
			Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
			Scheduler::add_parathread_claim(ParathreadClaim(thread_c, collator.clone()));

			run_to_block(2, |_| None);

			assert_eq!(Scheduler::scheduled().len(), 4);

			// cores 0, 1, 2, and 3 should be occupied. mark them as such.
			Scheduler::occupied(&[CoreIndex(0), CoreIndex(1), CoreIndex(2), CoreIndex(3)]);

			{
				let cores = AvailabilityCores::get();

				assert!(cores[0].is_some());
				assert!(cores[1].is_some());
				assert!(cores[2].is_some());
				assert!(cores[3].is_some());
				assert!(cores[4].is_none());

				assert!(Scheduler::scheduled().is_empty());
			}

			// add a couple more parathread claims - the claim on `b` will go to the 3rd parathread core (4)
			// and the claim on `d` will go back to the 1st parathread core (2). The claim on `e` then
			// will go for core `3`.
			Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));
			Scheduler::add_parathread_claim(ParathreadClaim(thread_d, collator.clone()));
			Scheduler::add_parathread_claim(ParathreadClaim(thread_e, collator.clone()));

			run_to_block(3, |_| None);

			{
				let scheduled = Scheduler::scheduled();

				// cores 0 and 1 are occupied by parachains. cores 2 and 3 are occupied by parathread
				// claims. core 4 was free.
				assert_eq!(scheduled.len(), 1);
				assert_eq!(scheduled[0], CoreAssignment {
					core: CoreIndex(4),
					para_id: thread_b,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(4),
				});
			}

			// now note that cores 0, 2, and 3 were freed.
			Scheduler::schedule(vec![
				(CoreIndex(0), FreedReason::Concluded),
				(CoreIndex(2), FreedReason::Concluded),
				(CoreIndex(3), FreedReason::TimedOut), // should go back on queue.
			]);

			{
				let scheduled = Scheduler::scheduled();

				// 1 thing scheduled before, + 3 cores freed.
				assert_eq!(scheduled.len(), 4);
				assert_eq!(scheduled[0], CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				});
				assert_eq!(scheduled[1], CoreAssignment {
					core: CoreIndex(2),
					para_id: thread_d,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(2),
				});
				assert_eq!(scheduled[2], CoreAssignment {
					core: CoreIndex(3),
					para_id: thread_e,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(3),
				});
				assert_eq!(scheduled[3], CoreAssignment {
					core: CoreIndex(4),
					para_id: thread_b,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(4),
				});

				// the prior claim on thread A concluded, but the claim on thread C was marked as
				// timed out.
				let index = ParathreadClaimIndex::get();
				let parathread_queue = ParathreadQueue::get();

				// thread A claim should have been wiped, but thread C claim should remain.
				assert_eq!(index, vec![thread_b, thread_c, thread_d, thread_e]);

				// Although C was descheduled, the core `4`  was occupied so C goes back on the queue.
				assert_eq!(parathread_queue.queue.len(), 1);
				assert_eq!(parathread_queue.queue[0], QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_c, collator.clone()),
						retries: 0, // retries not incremented by timeout - validators' fault.
					},
					core_offset: 2, // reassigned to next core. thread_e claim was on offset 1.
				});
			}
		});
	}

	#[test]
	fn schedule_rotates_groups() {
		let config = {
			let mut config = default_config();

			// make sure parathread requests don't retry-out
			config.parathread_retries = config.parachain_rotation_frequency * 3;
			config.parathread_cores = 2;
			config
		};

		let rotation_frequency = config.parachain_rotation_frequency;
		let parathread_cores = config.parathread_cores;

		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: config.clone(),
				..Default::default()
			},
			..Default::default()
		};

		let thread_a = ParaId::from(1);
		let thread_b = ParaId::from(2);

		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		let schedule_blank_para = |id, is_chain| Paras::schedule_para_initialize(id, ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: Vec::new().into(),
			parachain: is_chain,
		});

		new_test_ext(genesis_config).execute_with(|| {
			assert_eq!(default_config().parathread_cores, 3);

			schedule_blank_para(thread_a, false);
			schedule_blank_para(thread_b, false);

			// start a new session to activate, 5 validators for 5 cores.
			run_to_block(1, |number| match number {
				1 => Some(SessionChangeNotification {
					new_config: config.clone(),
					validators: vec![
						ValidatorId::from(Sr25519Keyring::Alice.public()),
						ValidatorId::from(Sr25519Keyring::Eve.public()),
					],
					..Default::default()
				}),
				_ => None,
			});

			let session_start_block = <Scheduler as Store>::SessionStartBlock::get();
			assert_eq!(session_start_block, 1);

			Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
			Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));

			run_to_block(2, |_| None);

			let assert_groups_rotated = |rotations: u32| {
				let scheduled = Scheduler::scheduled();
				assert_eq!(scheduled.len(), 2);
				assert_eq!(scheduled[0].group_idx, GroupIndex((0u32 + rotations) % parathread_cores));
				assert_eq!(scheduled[1].group_idx, GroupIndex((1u32 + rotations) % parathread_cores));
			};

			assert_groups_rotated(0);

			// one block before first rotation.
			run_to_block(rotation_frequency, |_| None);

			let rotations_since_session_start = (rotation_frequency - session_start_block) / rotation_frequency;
			assert_eq!(rotations_since_session_start, 0);
			assert_groups_rotated(0);

			// first rotation.
			run_to_block(rotation_frequency + 1, |_| None);
			assert_groups_rotated(1);

			// one block before second rotation.
			run_to_block(rotation_frequency * 2, |_| None);
			assert_groups_rotated(1);

			// second rotation.
			run_to_block(rotation_frequency * 2 + 1, |_| None);
			assert_groups_rotated(2);
		});
	}

	#[test]
	fn parathread_claims_are_pruned_after_retries() {
		let max_retries = default_config().parathread_retries;

		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};

		let thread_a = ParaId::from(1);
		let thread_b = ParaId::from(2);

		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		let schedule_blank_para = |id, is_chain| Paras::schedule_para_initialize(id, ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: Vec::new().into(),
			parachain: is_chain,
		});

		new_test_ext(genesis_config).execute_with(|| {
			assert_eq!(default_config().parathread_cores, 3);

			schedule_blank_para(thread_a, false);
			schedule_blank_para(thread_b, false);

			// start a new session to activate, 5 validators for 5 cores.
			run_to_block(1, |number| match number {
				1 => Some(SessionChangeNotification {
					new_config: default_config(),
					validators: vec![
						ValidatorId::from(Sr25519Keyring::Alice.public()),
						ValidatorId::from(Sr25519Keyring::Eve.public()),
					],
					..Default::default()
				}),
				_ => None,
			});

			Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
			Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));

			run_to_block(2, |_| None);
			assert_eq!(Scheduler::scheduled().len(), 2);

			run_to_block(2 + max_retries, |_| None);
			assert_eq!(Scheduler::scheduled().len(), 2);

			run_to_block(2 + max_retries + 1, |_| None);
			assert_eq!(Scheduler::scheduled().len(), 0);
		});
	}

	#[test]
	fn availability_predicate_works() {
		let genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		};

		let HostConfiguration {
			parachain_rotation_frequency,
			chain_availability_period,
			thread_availability_period,
			..
		} = default_config();
		let collator = CollatorId::from(Sr25519Keyring::Alice.public());

		assert!(chain_availability_period < thread_availability_period &&
			thread_availability_period < parachain_rotation_frequency);

		let chain_a = ParaId::from(1);
		let thread_a = ParaId::from(2);

		let schedule_blank_para = |id, is_chain| Paras::schedule_para_initialize(id, ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: Vec::new().into(),
			parachain: is_chain,
		});

		new_test_ext(genesis_config).execute_with(|| {
			schedule_blank_para(chain_a, true);
			schedule_blank_para(thread_a, false);

			// start a new session with our chain & thread registered.
			run_to_block(1, |number| match number {
				1 => Some(SessionChangeNotification {
					new_config: default_config(),
					validators: vec![
						ValidatorId::from(Sr25519Keyring::Alice.public()),
						ValidatorId::from(Sr25519Keyring::Bob.public()),
						ValidatorId::from(Sr25519Keyring::Charlie.public()),
						ValidatorId::from(Sr25519Keyring::Dave.public()),
						ValidatorId::from(Sr25519Keyring::Eve.public()),
					],
					..Default::default()
				}),
				_ => None,
			});

			// assign some availability cores.
			{
				AvailabilityCores::mutate(|cores| {
					cores[0] = Some(CoreOccupied::Parachain);
					cores[1] = Some(CoreOccupied::Parathread(ParathreadEntry {
						claim: ParathreadClaim(thread_a, collator),
						retries: 0,
					}))
				});
			}

			run_to_block(1 + thread_availability_period, |_| None);
			assert!(Scheduler::availability_timeout_predicate().is_none());

			run_to_block(1 + parachain_rotation_frequency, |_| None);

			{
				let pred = Scheduler::availability_timeout_predicate()
					.expect("predicate exists recently after rotation");

				let now = System::block_number();
				let would_be_timed_out = now - thread_availability_period;
				for i in 0..AvailabilityCores::get().len() {
					// returns true for unoccupied cores.
					// And can time out both threads and chains at this stage.
					assert!(pred(CoreIndex(i as u32), would_be_timed_out));
				}

				assert!(!pred(CoreIndex(0), now)); // assigned: chain
				assert!(!pred(CoreIndex(1), now)); // assigned: thread
				assert!(pred(CoreIndex(2), now));

				// check the tighter bound on chains vs threads.
				assert!(pred(CoreIndex(0), now - chain_availability_period));
				assert!(!pred(CoreIndex(1), now - chain_availability_period));

				// check the threshold is exact.
				assert!(!pred(CoreIndex(0), now - chain_availability_period + 1));
				assert!(!pred(CoreIndex(1), now - thread_availability_period + 1));
			}

			run_to_block(1 + parachain_rotation_frequency + chain_availability_period, |_| None);

			{
				let pred = Scheduler::availability_timeout_predicate()
					.expect("predicate exists recently after rotation");

				let would_be_timed_out = System::block_number() - thread_availability_period;

				assert!(!pred(CoreIndex(0), would_be_timed_out)); // chains can't be timed out now.
				assert!(pred(CoreIndex(1), would_be_timed_out)); // but threads can.
			}

			run_to_block(1 + parachain_rotation_frequency + thread_availability_period, |_| None);

			assert!(Scheduler::availability_timeout_predicate().is_none());
		});
	}
}
