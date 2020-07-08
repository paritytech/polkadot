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

//! The paras module is responsible for storing data on parachains and parathreads.
//!
//! It tracks which paras are parachains, what their current head data is in
//! this fork of the relay chain, what their validation code is, and what their past and upcoming
//! validation code is.
//!
//! A para is not considered live until it is registered and activated in this module. Activation can
//! only occur at session boundaries.

use sp_std::prelude::*;
use sp_std::marker::PhantomData;
use sp_runtime::traits::One;
use primitives::{
	parachain::{Id as ParaId, ValidationCode, HeadData},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
	traits::Get,
	weights::Weight,
};
use codec::{Encode, Decode};
use crate::{configuration, initializer::SessionChangeNotification};

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

pub trait Trait: system::Trait + configuration::Trait { }

// the two key times necessary to track for every code replacement.
#[derive(Default, Encode, Decode)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub struct ReplacementTimes<N> {
	/// The relay-chain block number that the code upgrade was expected to be activated.
	/// This is when the code change occurs from the para's perspective - after the
	/// first parablock included with a relay-parent with number >= this value.
	expected_at: N,
	/// The relay-chain block number at which the parablock activating the code upgrade was
	/// actually included. This means considered included and available, so this is the time at which
	/// that parablock enters the acceptance period in this fork of the relay-chain.
	activated_at: N,
}

/// Metadata used to track previous parachain validation code that we keep in
/// the state.
#[derive(Default, Encode, Decode)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub struct ParaPastCodeMeta<N> {
	/// Block numbers where the code was expected to be replaced and where the code
	/// was actually replaced, respectively. The first is used to do accurate lookups
	/// of historic code in historic contexts, whereas the second is used to do
	/// pruning on an accurate timeframe. These can be used as indices
	/// into the `PastCode` map along with the `ParaId` to fetch the code itself.
	upgrade_times: Vec<ReplacementTimes<N>>,
	/// Tracks the highest pruned code-replacement, if any. This is the `expected_at` value,
	/// not the `activated_at` value.
	last_pruned: Option<N>,
}

#[cfg_attr(test, derive(Debug, PartialEq))]
enum UseCodeAt<N> {
	/// Use the current code.
	Current,
	/// Use the code that was replaced at the given block number.
	/// This is an inclusive endpoint - a parablock in the context of a relay-chain block on this fork
	/// with number N should use the code that is replaced at N.
	ReplacedAt(N),
}

impl<N: Ord + Copy> ParaPastCodeMeta<N> {
	// note a replacement has occurred at a given block number.
	fn note_replacement(&mut self, expected_at: N, activated_at: N) {
		self.upgrade_times.push(ReplacementTimes { expected_at, activated_at })
	}

	// Yields an identifier that should be used for validating a
	// parablock in the context of a particular relay-chain block number.
	//
	// a return value of `None` means that there is no code we are aware of that
	// should be used to validate at the given height.
	#[allow(unused)]
	fn code_at(&self, para_at: N) -> Option<UseCodeAt<N>> {
		// Find out
		// a) if there is a point where code was replaced _after_ execution in this context and
		// b) what the index of that point is.
		let replaced_after_pos = self.upgrade_times.iter().position(|t| t.expected_at >= para_at);

		if let Some(replaced_after_pos) = replaced_after_pos {
			// The earliest stored code replacement needs to be special-cased, since we need to check
			// against the pruning state to see if this replacement represents the correct code, or
			// is simply after a replacement that actually represents the correct code, but has been pruned.
			let was_pruned = replaced_after_pos == 0
				&& self.last_pruned.map_or(false, |t| t >= para_at);

			if was_pruned {
				None
			} else {
				Some(UseCodeAt::ReplacedAt(self.upgrade_times[replaced_after_pos].expected_at))
			}
		} else {
			// No code replacements after this context.
			// This means either that the current code is valid, or `para_at` is so old that
			// we don't know the code necessary anymore. Compare against `last_pruned` to determine.
			self.last_pruned.as_ref().map_or(
				Some(UseCodeAt::Current), // nothing pruned, use current
				|t| if t >= &para_at { None } else { Some(UseCodeAt::Current) },
			)
		}
	}

	// The block at which the most recently tracked code change occurred, from the perspective
	// of the para.
	fn most_recent_change(&self) -> Option<N> {
		self.upgrade_times.last().map(|x| x.expected_at.clone())
	}

	// prunes all code upgrade logs occurring at or before `max`.
	// note that code replaced at `x` is the code used to validate all blocks before
	// `x`. Thus, `max` should be outside of the slashing window when this is invoked.
	//
	// Since we don't want to prune anything inside the acceptance period, and the parablock only
	// enters the acceptance period after being included, we prune based on the activation height of
	// the code change, not the expected height of the code change.
	//
	// returns an iterator of block numbers at which code was replaced, where the replaced
	// code should be now pruned, in ascending order.
	fn prune_up_to(&'_ mut self, max: N) -> impl Iterator<Item=N> + '_ {
		let to_prune = self.upgrade_times.iter().take_while(|t| t.activated_at <= max).count();
		let drained = if to_prune == 0 {
			// no-op prune.
			self.upgrade_times.drain(self.upgrade_times.len()..)
		} else {
			// if we are actually pruning something, update the last_pruned member.
			self.last_pruned = Some(self.upgrade_times[to_prune - 1].expected_at);
			self.upgrade_times.drain(..to_prune)
		};

		drained.map(|times| times.expected_at)
	}
}

/// Arguments for initializing a para.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct ParaGenesisArgs {
	/// The initial head data to use.
	pub genesis_head: HeadData,
	/// The initial validation code to use.
	pub validation_code: ValidationCode,
	/// True if parachain, false if parathread.
	pub parachain: bool,
}


decl_storage! {
	trait Store for Module<T: Trait> as Paras {
		/// All parachains. Ordered ascending by ParaId. Parathreads are not included.
		Parachains get(fn parachains): Vec<ParaId>;
		/// All parathreads.
		Parathreads: map hasher(twox_64_concat) ParaId => Option<()>;
		/// The head-data of every registered para.
		Heads get(fn para_head): map hasher(twox_64_concat) ParaId => Option<HeadData>;
		/// The validation code of every live para.
		CurrentCode get(fn current_code): map hasher(twox_64_concat) ParaId => Option<ValidationCode>;
		/// Actual past code, indicated by the para id as well as the block number at which it became outdated.
		PastCode: map hasher(twox_64_concat) (ParaId, T::BlockNumber) => Option<ValidationCode>;
		/// Past code of parachains. The parachains themselves may not be registered anymore,
		/// but we also keep their code on-chain for the same amount of time as outdated code
		/// to keep it available for secondary checkers.
		PastCodeMeta get(fn past_code_meta):
			map hasher(twox_64_concat) ParaId => ParaPastCodeMeta<T::BlockNumber>;
		/// Which paras have past code that needs pruning and the relay-chain block at which the code was replaced.
		/// Note that this is the actual height of the included block, not the expected height at which the
		/// code upgrade would be applied, although they may be equal.
		/// This is to ensure the entire acceptance period is covered, not an offset acceptance period starting
		/// from the time at which the parachain perceives a code upgrade as having occurred.
		/// Multiple entries for a single para are permitted. Ordered ascending by block number.
		PastCodePruning: Vec<(ParaId, T::BlockNumber)>;
		/// The block number at which the planned code change is expected for a para.
		/// The change will be applied after the first parablock for this ID included which executes
		/// in the context of a relay chain block with a number >= `expected_at`.
		FutureCodeUpgrades get(fn future_code_upgrade_at): map hasher(twox_64_concat) ParaId => Option<T::BlockNumber>;
		/// The actual future code of a para.
		FutureCode: map hasher(twox_64_concat) ParaId => Option<ValidationCode>;

		/// Upcoming paras (chains and threads). These are only updated on session change. Corresponds to an
		/// entry in the upcoming-genesis map.
		UpcomingParas: Vec<ParaId>;
		/// Upcoming paras instantiation arguments.
		UpcomingParasGenesis: map hasher(twox_64_concat) ParaId => Option<ParaGenesisArgs>;
		/// Paras that are to be cleaned up at the end of the session.
		OutgoingParas: Vec<ParaId>;

	}
	add_extra_genesis {
		config(paras): Vec<(ParaId, ParaGenesisArgs)>;
		config(_phdata): PhantomData<T>;
		build(build::<T>);
	}
}

#[cfg(feature = "std")]
fn build<T: Trait>(config: &GenesisConfig<T>) {
	let mut parachains: Vec<_> = config.paras
		.iter()
		.filter(|(_, args)| args.parachain)
		.map(|&(ref id, _)| id)
		.cloned()
		.collect();

	parachains.sort_unstable();
	parachains.dedup();

	Parachains::put(&parachains);

	for (id, genesis_args) in &config.paras {
		<Module<T> as Store>::CurrentCode::insert(&id, &genesis_args.validation_code);
		<Module<T> as Store>::Heads::insert(&id, &genesis_args.genesis_head);
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The parachains configuration module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin, system = system {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {
	/// Called by the initializer to initialize the configuration module.
	pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
		Self::prune_old_code(now)
	}

	/// Called by the initializer to finalize the configuration module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(_notification: &SessionChangeNotification<T::BlockNumber>) {
		let now = <system::Module<T>>::block_number();
		let mut parachains = Self::clean_up_outgoing(now);
		Self::apply_incoming(&mut parachains);
		<Self as Store>::Parachains::set(parachains);
	}

	/// Cleans up all outgoing paras. Returns the new set of parachains
	fn clean_up_outgoing(now: T::BlockNumber) -> Vec<ParaId> {
		let mut parachains = <Self as Store>::Parachains::get();
		let outgoing = <Self as Store>::OutgoingParas::take();

		for outgoing_para in outgoing {
			if let Ok(i) = parachains.binary_search(&outgoing_para) {
				parachains.remove(i);
			} else {
				<Self as Store>::Parathreads::remove(&outgoing_para);
			}

			<Self as Store>::Heads::remove(&outgoing_para);
			<Self as Store>::FutureCodeUpgrades::remove(&outgoing_para);
			<Self as Store>::FutureCode::remove(&outgoing_para);

			let removed_code = <Self as Store>::CurrentCode::take(&outgoing_para);
			if let Some(removed_code) = removed_code {
				Self::note_past_code(outgoing_para, now, now, removed_code);
			}
		}

		parachains
	}

	/// Applies all incoming paras, updating the parachains list for those that are parachains.
	fn apply_incoming(parachains: &mut Vec<ParaId>) {
		let upcoming = <Self as Store>::UpcomingParas::take();
		for upcoming_para in upcoming {
			let genesis_data = match <Self as Store>::UpcomingParasGenesis::take(&upcoming_para) {
				None => continue,
				Some(g) => g,
			};

			if genesis_data.parachain {
				if let Err(i) = parachains.binary_search(&upcoming_para) {
					parachains.insert(i, upcoming_para);
				}
			} else {
				<Self as Store>::Parathreads::insert(&upcoming_para, ());
			}

			<Self as Store>::Heads::insert(&upcoming_para, genesis_data.genesis_head);
			<Self as Store>::CurrentCode::insert(&upcoming_para, genesis_data.validation_code);
		}
	}

	// note replacement of the code of para with given `id`, which occured in the
	// context of the given relay-chain block number. provide the replaced code.
	//
	// `at` for para-triggered replacement is the block number of the relay-chain
	// block in whose context the parablock was executed
	// (i.e. number of `relay_parent` in the receipt)
	fn note_past_code(
		id: ParaId,
		at: T::BlockNumber,
		now: T::BlockNumber,
		old_code: ValidationCode,
	) -> Weight {

		<Self as Store>::PastCodeMeta::mutate(&id, |past_meta| {
			past_meta.note_replacement(at, now);
		});

		<Self as Store>::PastCode::insert(&(id, at), old_code);

		// Schedule pruning for this past-code to be removed as soon as it
		// exits the slashing window.
		<Self as Store>::PastCodePruning::mutate(|pruning| {
			let insert_idx = pruning.binary_search_by_key(&at, |&(_, b)| b)
				.unwrap_or_else(|idx| idx);
			pruning.insert(insert_idx, (id, now));
		});

		T::DbWeight::get().reads_writes(2, 3)
	}

	// looks at old code metadata, compares them to the current acceptance window, and prunes those
	// that are too old.
	fn prune_old_code(now: T::BlockNumber) -> Weight {
		let config = configuration::Module::<T>::config();
		let acceptance_period = config.acceptance_period;
		if now <= acceptance_period {
			let weight = T::DbWeight::get().reads_writes(1, 0);
			return weight;
		}

		// The height of any changes we no longer should keep around.
		let pruning_height = now - (acceptance_period + One::one());

		let pruning_tasks_done =
			<Self as Store>::PastCodePruning::mutate(|pruning_tasks: &mut Vec<(_, T::BlockNumber)>| {
				let (pruning_tasks_done, pruning_tasks_to_do) = {
					// find all past code that has just exited the pruning window.
					let up_to_idx = pruning_tasks.iter()
						.take_while(|&(_, at)| at <= &pruning_height)
						.count();
					(up_to_idx, pruning_tasks.drain(..up_to_idx))
				};

				for (para_id, _) in pruning_tasks_to_do {
					let full_deactivate = <Self as Store>::PastCodeMeta::mutate(&para_id, |meta| {
						for pruned_repl_at in meta.prune_up_to(pruning_height) {
							<Self as Store>::PastCode::remove(&(para_id, pruned_repl_at));
						}

						meta.most_recent_change().is_none() && Self::para_head(&para_id).is_none()
					});

					// This parachain has been removed and now the vestigial code
					// has been removed from the state. clean up meta as well.
					if full_deactivate {
						<Self as Store>::PastCodeMeta::remove(&para_id);
					}
				}

				pruning_tasks_done as u64
			});

		// 1 read for the meta for each pruning task, 1 read for the config
		// 2 writes: updating the meta and pruning the code
		T::DbWeight::get().reads_writes(1 + pruning_tasks_done, 2 * pruning_tasks_done)
	}

	/// Schedule a para to be initialized at the start of the next session.
	#[allow(unused)]
	pub(crate) fn schedule_para_initialize(id: ParaId, genesis: ParaGenesisArgs) -> Weight {
		let dup = UpcomingParas::mutate(|v| {
			match v.binary_search(&id) {
				Ok(_) => true,
				Err(i) => {
					v.insert(i, id);
					false
				}
			}
		});

		if dup {
			let weight = T::DbWeight::get().reads_writes(1, 0);
			return weight;
		}

		UpcomingParasGenesis::insert(&id, &genesis);

		T::DbWeight::get().reads_writes(1, 2)
	}

	/// Schedule a para to be cleaned up at the start of the next session.
	#[allow(unused)]
	pub(crate) fn schedule_para_cleanup(id: ParaId) -> Weight {
		OutgoingParas::mutate(|v| {
			match v.binary_search(&id) {
				Ok(_) => T::DbWeight::get().reads_writes(1, 0),
				Err(i) => {
					v.insert(i, id);
					T::DbWeight::get().reads_writes(1, 1)
				}
			}
		})
	}

	/// Schedule a future code upgrade of the given parachain, to be applied after inclusion
	/// of a block of the same parachain executed in the context of a relay-chain block
	/// with number >= `expected_at`
	///
	/// If there is already a scheduled code upgrade for the para, this is a no-op.
	pub(crate) fn schedule_code_upgrade(
		id: ParaId,
		new_code: ValidationCode,
		expected_at: T::BlockNumber,
	) -> Weight {
		<Self as Store>::FutureCodeUpgrades::mutate(&id, |up| {
			if up.is_some() {
				T::DbWeight::get().reads_writes(1, 0)
			} else {
				*up = Some(expected_at);
				FutureCode::insert(&id, new_code);
				T::DbWeight::get().reads_writes(1, 2)
			}
		})
	}

	/// Note that a para has progressed to a new head, where the new head was executed in the context
	/// of a relay-chain block with given number. This will apply pending code upgrades based
	/// on the block number provided.
	pub(crate) fn note_new_head(
		id: ParaId,
		new_head: HeadData,
		execution_context: T::BlockNumber,
	) -> Weight {
		Heads::insert(&id, new_head);

		if let Some(expected_at) = <Self as Store>::FutureCodeUpgrades::get(&id) {
			if expected_at <= execution_context {
				<Self as Store>::FutureCodeUpgrades::remove(&id);

				// Both should always be `Some` in this case, since a code upgrade is scheduled.
				let new_code = FutureCode::take(&id).unwrap_or_default();
				let prior_code = CurrentCode::get(&id).unwrap_or_default();
				CurrentCode::insert(&id, &new_code);

				// `now` is only used for registering pruning as part of `fn note_past_code`
				let now = <system::Module<T>>::block_number();

				let weight = Self::note_past_code(
					id,
					expected_at,
					now,
					prior_code,
				);

				// add 1 to writes due to heads update.
				weight + T::DbWeight::get().reads_writes(3, 1 + 3)
			} else {
				T::DbWeight::get().reads_writes(1, 1 + 0)
			}
		} else {
			T::DbWeight::get().reads_writes(1, 1)
		}
	}

	/// Fetches the validation code to be used when validating a block in the context of the given
	/// relay-chain height. A second block number parameter may be used to tell the lookup to proceed
	/// as if an intermediate parablock has been with the given relay-chain height as its context.
	/// This may return past, current, or (with certain choices of `assume_intermediate`) future code.
	///
	/// `assume_intermediate`, if provided, must be before `at`. This will return `None` if the validation
	/// code has been pruned.
	#[allow(unused)]
	pub(crate) fn validation_code_at(
		id: ParaId,
		at: T::BlockNumber,
		assume_intermediate: Option<T::BlockNumber>,
	) -> Option<ValidationCode> {
		let now = <system::Module<T>>::block_number();
		let config = <configuration::Module<T>>::config();

		if assume_intermediate.as_ref().map_or(false, |i| &at <= i) {
			return None;
		}

		let planned_upgrade = <Self as Store>::FutureCodeUpgrades::get(&id);
		let upgrade_applied_intermediate = match assume_intermediate {
			Some(a) => planned_upgrade.as_ref().map_or(false, |u| u <= &a),
			None => false,
		};

		if upgrade_applied_intermediate {
			FutureCode::get(&id)
		} else {
			match Self::past_code_meta(&id).code_at(at) {
				None => None,
				Some(UseCodeAt::Current) => CurrentCode::get(&id),
				Some(UseCodeAt::ReplacedAt(replaced)) => <Self as Store>::PastCode::get(&(id, replaced))
			}
		}
	}

	/// Whether a para ID corresponds to any live parathread.
	pub(crate) fn is_parathread(id: ParaId) -> bool {
		Parathreads::get(&id).is_some()
	}

	/// The block number of the last scheduled upgrade of the requested para. Includes future upgrades
	/// if the flag is set. This is the `expected_at` number, not the `activated_at` number.
	pub(crate) fn last_code_upgrade(id: ParaId, include_future: bool) -> Option<T::BlockNumber> {
		if include_future {
			if let Some(at) = Self::future_code_upgrade_at(id) {
				return Some(at);
			}
		}

		Self::past_code_meta(&id).most_recent_change()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use primitives::BlockNumber;
	use frame_support::traits::{OnFinalize, OnInitialize};

	use crate::mock::{new_test_ext, Paras, System, GenesisConfig as MockGenesisConfig};
	use crate::configuration::HostConfiguration;

	fn run_to_block(to: BlockNumber, new_session: Option<Vec<BlockNumber>>) {
		while System::block_number() < to {
			let b = System::block_number();
			Paras::initializer_finalize();
			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			if new_session.as_ref().map_or(false, |v| v.contains(&(b + 1))) {
				Paras::initializer_on_new_session(&Default::default());
			}
			Paras::initializer_initialize(b + 1);
		}
	}

	fn upgrade_at(expected_at: BlockNumber, activated_at: BlockNumber) -> ReplacementTimes<BlockNumber> {
		ReplacementTimes { expected_at, activated_at }
	}

	#[test]
	fn para_past_code_meta_gives_right_code() {
		let mut past_code = ParaPastCodeMeta::default();
		assert_eq!(past_code.code_at(0u32), Some(UseCodeAt::Current));

		past_code.note_replacement(10, 12);
		assert_eq!(past_code.code_at(0), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(10), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(11), Some(UseCodeAt::Current));

		past_code.note_replacement(20, 25);
		assert_eq!(past_code.code_at(1), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(10), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(11), Some(UseCodeAt::ReplacedAt(20)));
		assert_eq!(past_code.code_at(20), Some(UseCodeAt::ReplacedAt(20)));
		assert_eq!(past_code.code_at(21), Some(UseCodeAt::Current));

		past_code.last_pruned = Some(5);
		assert_eq!(past_code.code_at(1), None);
		assert_eq!(past_code.code_at(5), None);
		assert_eq!(past_code.code_at(6), Some(UseCodeAt::ReplacedAt(10)));
	}

	#[test]
	fn para_past_code_pruning_works_correctly() {
		let mut past_code = ParaPastCodeMeta::default();
		past_code.note_replacement(10u32, 10);
		past_code.note_replacement(20, 25);
		past_code.note_replacement(30, 35);

		let old = past_code.clone();
		assert!(past_code.prune_up_to(9).collect::<Vec<_>>().is_empty());
		assert_eq!(old, past_code);

		assert_eq!(past_code.prune_up_to(10).collect::<Vec<_>>(), vec![10]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![upgrade_at(20, 25), upgrade_at(30, 35)],
			last_pruned: Some(10),
		});

		assert!(past_code.prune_up_to(21).collect::<Vec<_>>().is_empty());

		assert_eq!(past_code.prune_up_to(26).collect::<Vec<_>>(), vec![20]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![upgrade_at(30, 35)],
			last_pruned: Some(20),
		});

		past_code.note_replacement(40, 42);
		past_code.note_replacement(50, 53);
		past_code.note_replacement(60, 66);

		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![upgrade_at(30, 35), upgrade_at(40, 42), upgrade_at(50, 53), upgrade_at(60, 66)],
			last_pruned: Some(20),
		});

		assert_eq!(past_code.prune_up_to(60).collect::<Vec<_>>(), vec![30, 40, 50]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![upgrade_at(60, 66)],
			last_pruned: Some(50),
		});

		assert_eq!(past_code.prune_up_to(66).collect::<Vec<_>>(), vec![60]);

		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: Vec::new(),
			last_pruned: Some(60),
		});
	}

	#[test]
	fn para_past_code_pruning_in_initialize() {
		let acceptance_period = 10;
		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: Default::default(),
			}),
			(1u32.into(), ParaGenesisArgs {
				parachain: false,
				genesis_head: Default::default(),
				validation_code: Default::default(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let id = ParaId::from(0u32);
			let at_block: BlockNumber = 10;
			let included_block: BlockNumber = 12;

			<Paras as Store>::PastCode::insert(&(id, at_block), &ValidationCode(vec![1, 2, 3]));
			<Paras as Store>::PastCodePruning::put(&vec![(id, included_block)]);

			{
				let mut code_meta = Paras::past_code_meta(&id);
				code_meta.note_replacement(at_block, included_block);
				<Paras as Store>::PastCodeMeta::insert(&id, &code_meta);
			}

			let pruned_at: BlockNumber = included_block + acceptance_period + 1;
			assert_eq!(<Paras as Store>::PastCode::get(&(id, at_block)), Some(vec![1, 2, 3].into()));

			run_to_block(pruned_at - 1, None);
			assert_eq!(<Paras as Store>::PastCode::get(&(id, at_block)), Some(vec![1, 2, 3].into()));
			assert_eq!(Paras::past_code_meta(&id).most_recent_change(), Some(at_block));

			run_to_block(pruned_at, None);
			assert!(<Paras as Store>::PastCode::get(&(id, at_block)).is_none());
			assert!(Paras::past_code_meta(&id).most_recent_change().is_none());
		});
	}

	#[test]
	fn note_new_head_sets_head() {
		let acceptance_period = 10;
		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: Default::default(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let id_a = ParaId::from(0u32);

			assert_eq!(Paras::para_head(&id_a), Some(Default::default()));

			Paras::note_new_head(id_a, vec![1, 2, 3].into(), 0);

			assert_eq!(Paras::para_head(&id_a), Some(vec![1, 2, 3].into()));
		});
	}

	#[test]
	fn note_past_code_sets_up_pruning_correctly() {
		let acceptance_period = 10;
		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: Default::default(),
			}),
			(1u32.into(), ParaGenesisArgs {
				parachain: false,
				genesis_head: Default::default(),
				validation_code: Default::default(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let id_a = ParaId::from(0u32);
			let id_b = ParaId::from(1u32);

			Paras::note_past_code(id_a, 10, 12, vec![1, 2, 3].into());
			Paras::note_past_code(id_b, 20, 23, vec![4, 5, 6].into());

			assert_eq!(<Paras as Store>::PastCodePruning::get(), vec![(id_a, 12), (id_b, 23)]);
			assert_eq!(
				Paras::past_code_meta(&id_a),
				ParaPastCodeMeta {
					upgrade_times: vec![upgrade_at(10, 12)],
					last_pruned: None,
				}
			);
			assert_eq!(
				Paras::past_code_meta(&id_b),
				ParaPastCodeMeta {
					upgrade_times: vec![upgrade_at(20, 23)],
					last_pruned: None,
				}
			);
		});
	}

	#[test]
	fn code_upgrade_applied_after_delay() {
		let acceptance_period = 10;
		let validation_upgrade_delay = 5;

		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: vec![1, 2, 3].into(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					validation_upgrade_delay,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2, None);
			assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

			let expected_at = {
				// this parablock is in the context of block 1.
				let expected_at = 1 + validation_upgrade_delay;
				Paras::schedule_code_upgrade(para_id, new_code.clone(), expected_at);
				Paras::note_new_head(para_id, Default::default(), 1);

				assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
				assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));
				assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

				expected_at
			};

			run_to_block(expected_at, None);

			// the candidate is in the context of the parent of `expected_at`,
			// thus does not trigger the code upgrade.
			{
				Paras::note_new_head(para_id, Default::default(), expected_at - 1);

				assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
				assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));
				assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));
			}

			run_to_block(expected_at + 1, None);

			// the candidate is in the context of `expected_at`, and triggers
			// the upgrade.
			{
				Paras::note_new_head(para_id, Default::default(), expected_at);

				assert_eq!(
					Paras::past_code_meta(&para_id).most_recent_change(),
					Some(expected_at),
				);
				assert_eq!(
					<Paras as Store>::PastCode::get(&(para_id, expected_at)),
					Some(vec![1, 2, 3,].into()),
				);
				assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
				assert!(<Paras as Store>::FutureCode::get(&para_id).is_none());
				assert_eq!(Paras::current_code(&para_id), Some(new_code));
			}
		});
	}

	#[test]
	fn code_upgrade_applied_after_delay_even_when_late() {
		let acceptance_period = 10;
		let validation_upgrade_delay = 5;

		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: vec![1, 2, 3].into(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					validation_upgrade_delay,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2, None);
			assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

			let expected_at = {
				// this parablock is in the context of block 1.
				let expected_at = 1 + validation_upgrade_delay;
				Paras::schedule_code_upgrade(para_id, new_code.clone(), expected_at);
				Paras::note_new_head(para_id, Default::default(), 1);

				assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
				assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));
				assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

				expected_at
			};

			run_to_block(expected_at + 1 + 4, None);

			// the candidate is in the context of the first descendent of `expected_at`, and triggers
			// the upgrade.
			{
				Paras::note_new_head(para_id, Default::default(), expected_at + 4);

				assert_eq!(
					Paras::past_code_meta(&para_id).most_recent_change(),
					Some(expected_at),
				);
				assert_eq!(
					<Paras as Store>::PastCode::get(&(para_id, expected_at)),
					Some(vec![1, 2, 3,].into()),
				);
				assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
				assert!(<Paras as Store>::FutureCode::get(&para_id).is_none());
				assert_eq!(Paras::current_code(&para_id), Some(new_code));
			}
		});
	}

	#[test]
	fn submit_code_change_when_not_allowed_is_err() {
		let acceptance_period = 10;

		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: vec![1, 2, 3].into(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);
			let newer_code = ValidationCode(vec![4, 5, 6, 7]);

			run_to_block(1, None);

			Paras::schedule_code_upgrade(para_id, new_code.clone(), 8);
			assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(8));
			assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));

			Paras::schedule_code_upgrade(para_id, newer_code.clone(), 10);
			assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(8));
			assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));
		});
	}

	#[test]
	fn full_parachain_cleanup_storage() {
		let acceptance_period = 10;

		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: vec![1, 2, 3].into(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2, None);
			assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

			let expected_at = {
				// this parablock is in the context of block 1.
				let expected_at = 1 + 5;
				Paras::schedule_code_upgrade(para_id, new_code.clone(), expected_at);
				Paras::note_new_head(para_id, Default::default(), 1);

				assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
				assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));
				assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

				expected_at
			};

			Paras::schedule_para_cleanup(para_id);

			// Just scheduling cleanup shouldn't change anything.
			{
				assert_eq!(<Paras as Store>::OutgoingParas::get(), vec![para_id]);
				assert_eq!(Paras::parachains(), vec![para_id]);

				assert!(Paras::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(<Paras as Store>::FutureCodeUpgrades::get(&para_id), Some(expected_at));
				assert_eq!(<Paras as Store>::FutureCode::get(&para_id), Some(new_code.clone()));
				assert_eq!(Paras::current_code(&para_id), Some(vec![1, 2, 3].into()));

				assert_eq!(<Paras as Store>::Heads::get(&para_id), Some(Default::default()));
			}

			// run to block, with a session change at that block.
			run_to_block(3, Some(vec![3]));

			// cleaning up the parachain should place the current parachain code
			// into the past code buffer & schedule cleanup.
			assert_eq!(Paras::past_code_meta(&para_id).most_recent_change(), Some(3));
			assert_eq!(<Paras as Store>::PastCode::get(&(para_id, 3)), Some(vec![1, 2, 3].into()));
			assert_eq!(<Paras as Store>::PastCodePruning::get(), vec![(para_id, 3)]);

			// any future upgrades haven't been used to validate yet, so those
			// are cleaned up immediately.
			assert!(<Paras as Store>::FutureCodeUpgrades::get(&para_id).is_none());
			assert!(<Paras as Store>::FutureCode::get(&para_id).is_none());
			assert!(Paras::current_code(&para_id).is_none());

			// run to do the final cleanup
			let cleaned_up_at = 3 + acceptance_period + 1;
			run_to_block(cleaned_up_at, None);

			// now the final cleanup: last past code cleaned up, and this triggers meta cleanup.
			assert_eq!(Paras::past_code_meta(&para_id), Default::default());
			assert!(<Paras as Store>::PastCode::get(&(para_id, 3)).is_none());
			assert!(<Paras as Store>::PastCodePruning::get().is_empty());
		});
	}

	#[test]
	fn para_incoming_at_session() {
		new_test_ext(Default::default()).execute_with(|| {
			run_to_block(1, None);

			let b = ParaId::from(525);
			let a = ParaId::from(999);
			let c = ParaId::from(333);

			Paras::schedule_para_initialize(
				b,
				ParaGenesisArgs {
					parachain: true,
					genesis_head: vec![1].into(),
					validation_code: vec![1].into(),
				},
			);

			Paras::schedule_para_initialize(
				a,
				ParaGenesisArgs {
					parachain: false,
					genesis_head: vec![2].into(),
					validation_code: vec![2].into(),
				},
			);

			Paras::schedule_para_initialize(
				c,
				ParaGenesisArgs {
					parachain: true,
					genesis_head: vec![3].into(),
					validation_code: vec![3].into(),
				},
			);

			assert_eq!(<Paras as Store>::UpcomingParas::get(), vec![c, b, a]);
			assert!(<Paras as Store>::Parathreads::get(&a).is_none());


			// run to block without session change.
			run_to_block(2, None);

			assert_eq!(Paras::parachains(), Vec::new());
			assert_eq!(<Paras as Store>::UpcomingParas::get(), vec![c, b, a]);
			assert!(<Paras as Store>::Parathreads::get(&a).is_none());


			run_to_block(3, Some(vec![3]));

			assert_eq!(Paras::parachains(), vec![c, b]);
			assert_eq!(<Paras as Store>::UpcomingParas::get(), Vec::new());

			assert!(<Paras as Store>::Parathreads::get(&a).is_some());

			assert_eq!(Paras::current_code(&a), Some(vec![2].into()));
			assert_eq!(Paras::current_code(&b), Some(vec![1].into()));
			assert_eq!(Paras::current_code(&c), Some(vec![3].into()));
		})
	}

	#[test]
	fn code_at_with_intermediate() {
		let acceptance_period = 10;

		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: vec![1, 2, 3].into(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let para_id = ParaId::from(0);
			let old_code: ValidationCode = vec![1, 2, 3].into();
			let new_code: ValidationCode = vec![4, 5, 6].into();
			Paras::schedule_code_upgrade(para_id, new_code.clone(), 10);

			// no intermediate, falls back on current/past.
			assert_eq!(Paras::validation_code_at(para_id, 1, None), Some(old_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 10, None), Some(old_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 100, None), Some(old_code.clone()));

			// intermediate before upgrade meant to be applied, falls back on current.
			assert_eq!(Paras::validation_code_at(para_id, 9, Some(8)), Some(old_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 10, Some(9)), Some(old_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 11, Some(9)), Some(old_code.clone()));

			// intermediate at or after upgrade applied
			assert_eq!(Paras::validation_code_at(para_id, 11, Some(10)), Some(new_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 100, Some(11)), Some(new_code.clone()));

			run_to_block(acceptance_period + 5, None);

			// at <= intermediate not allowed
			assert_eq!(Paras::validation_code_at(para_id, 10, Some(10)), None);
			assert_eq!(Paras::validation_code_at(para_id, 9, Some(10)), None);
		});
	}

	#[test]
	fn code_at_returns_up_to_end_of_acceptance_period() {
		let acceptance_period = 10;

		let paras = vec![
			(0u32.into(), ParaGenesisArgs {
				parachain: true,
				genesis_head: Default::default(),
				validation_code: vec![1, 2, 3].into(),
			}),
		];

		let genesis_config = MockGenesisConfig {
			paras: GenesisConfig { paras, ..Default::default() },
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(genesis_config).execute_with(|| {
			let para_id = ParaId::from(0);
			let old_code: ValidationCode = vec![1, 2, 3].into();
			let new_code: ValidationCode = vec![4, 5, 6].into();
			Paras::schedule_code_upgrade(para_id, new_code.clone(), 2);

			run_to_block(10, None);
			Paras::note_new_head(para_id, Default::default(), 7);

			assert_eq!(
				Paras::past_code_meta(&para_id).upgrade_times,
				vec![upgrade_at(2, 10)],
			);

			assert_eq!(Paras::validation_code_at(para_id, 2, None), Some(old_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 3, None), Some(new_code.clone()));

			run_to_block(10 + acceptance_period, None);

			assert_eq!(Paras::validation_code_at(para_id, 2, None), Some(old_code.clone()));
			assert_eq!(Paras::validation_code_at(para_id, 3, None), Some(new_code.clone()));

			run_to_block(10 + acceptance_period + 1, None);

			// code entry should be pruned now.

			assert_eq!(
				Paras::past_code_meta(&para_id),
				ParaPastCodeMeta {
					upgrade_times: Vec::new(),
					last_pruned: Some(2),
				},
			);

			assert_eq!(Paras::validation_code_at(para_id, 2, None), None); // pruned :(
			assert_eq!(Paras::validation_code_at(para_id, 3, None), Some(new_code.clone()));
		});
	}
}
