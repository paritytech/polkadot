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
use primitives::{
	parachain::{ValidatorId, Id as ParaId, ValidationCode, HeadData},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
	dispatch::DispatchResult,
	weights::{DispatchClass, Weight},
};
use codec::{Encode, Decode};
use system::ensure_root;
use crate::configuration;

pub trait Trait: system::Trait + configuration::Trait { }

/// Metadata used to track previous parachain validation code that we keep in
/// the state.
#[derive(Default, Encode, Decode)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub struct ParaPastCodeMeta<N> {
	// Block numbers where the code was replaced. These can be used as indices
	// into the `PastCode` map along with the `ParaId` to fetch the code itself.
	upgrade_times: Vec<N>,
	// This tracks the highest pruned code-replacement, if any.
	last_pruned: Option<N>,
}

#[cfg_attr(test, derive(Debug, PartialEq))]
enum UseCodeAt<N> {
	// Use the current code.
	Current,
	// Use the code that was replaced at the given block number.
	ReplacedAt(N),
}

impl<N: Ord + Copy> ParaPastCodeMeta<N> {
	// note a replacement has occurred at a given block number.
	fn note_replacement(&mut self, at: N) {
		self.upgrade_times.insert(0, at)
	}

	// Yields the block number of the code that should be used for validating at
	// the given block number.
	//
	// a return value of `None` means that there is no code we are aware of that
	// should be used to validate at the given height.
	fn code_at(&self, at: N) -> Option<UseCodeAt<N>> {
		// The `PastCode` map stores the code which was replaced at `t`.
		let end_position = self.upgrade_times.iter().position(|&t| t < at);
		if let Some(end_position) = end_position {
			Some(if end_position != 0 {
				// `end_position` gives us the replacement time where the code used at `at`
				// was set. But that code has been replaced: `end_position - 1` yields
				// that index.
				UseCodeAt::ReplacedAt(self.upgrade_times[end_position - 1])
			} else {
				// the most recent tracked replacement is before `at`.
				// this means that the code put in place then (i.e. the current code)
				// is correct for validating at `at`.
				UseCodeAt::Current
			})
		} else {
			if self.last_pruned.as_ref().map_or(true, |&n| n < at) {
				// Our `last_pruned` is before `at`, so we still have the code!
				// but no code upgrade entries found before the `at` parameter.
				//
				// this means one of two things is true:
				// 1. there are no non-pruned upgrade logs. in this case use `Current`
				// 2. there are non-pruned upgrade logs all after `at`.
				//    in this case use the oldest upgrade log.
				Some(self.upgrade_times.last()
					.map(|n| UseCodeAt::ReplacedAt(*n))
					.unwrap_or(UseCodeAt::Current)
				)
			} else {
				// We don't have the code anymore.
				None
			}
		}
	}

	// The block at which the most recently tracked code change occurred.
	fn most_recent_change(&self) -> Option<N> {
		self.upgrade_times.first().map(|x| x.clone())
	}

	// prunes all code upgrade logs occurring at or before `max`.
	// note that code replaced at `x` is the code used to validate all blocks before
	// `x`. Thus, `max` should be outside of the slashing window when this is invoked.
	//
	// returns an iterator of block numbers at which code was replaced, where the replaced
	// code should be now pruned, in ascending order.
	fn prune_up_to(&'_ mut self, max: N) -> impl Iterator<Item=N> + '_ {
		match self.upgrade_times.iter().position(|&t| t <= max) {
			None => {
				// this is a no-op `drain` - desired because all
				// logged code upgrades occurred after `max`.
				self.upgrade_times.drain(self.upgrade_times.len()..).rev()
			}
			Some(pos) => {
				self.last_pruned = Some(self.upgrade_times[pos]);
				self.upgrade_times.drain(pos..).rev()
			}
		}
	}
}

/// Arguments for initializing a para.
#[derive(Encode, Decode)]
pub struct ParaGenesisArgs {
	/// The initial head data to use.
	genesis_head: HeadData,
	/// The initial validation code to use.
	validation_code: ValidationCode,
	/// True if parachain, false if parathread.
	parachain: bool,
}


decl_storage! {
	trait Store for Module<T: Trait> as Paras {
		/// All parachains. Ordered ascending by ParaId. Parathreads are not included.
		Parachains: Vec<ParaId>;
		/// The head-data of every registered para.
		Heads: map hasher(twox_64_concat) ParaId => Option<HeadData>;
		/// The validation code of every live para.
		CurrentCode: map hasher(twox_64_concat) ParaId => Option<ValidationCode>;
		/// Actual past code, indicated by the para id as well as the block number at which it became outdated.
		PastCode: map hasher(twox_64_concat) (ParaId, T::BlockNumber) => Option<ValidationCode>;
		/// Past code of parachains. The parachains themselves may not be registered anymore,
		/// but we also keep their code on-chain for the same amount of time as outdated code
		/// to keep it available for secondary checkers.
		PastCodeMeta: map hasher(twox_64_concat) ParaId => ParaPastCodeMeta<T::BlockNumber>;
		/// Which paras have past code that needs pruning and the relay-chain block in which context the code was replaced.
		/// Multiple entries for a single para are permitted. Ordered ascending by block number.
		PastCodePruning: Vec<(ParaId, T::BlockNumber)>;
		/// The block number at which the planned code change is expected for a para.
		/// The change will be applied after the first parablock for this ID included which executes
		/// in the context of a relay chain block with a number >= `expected_at`.
		FutureCodeUpgrades: map hasher(twox_64_concat) ParaId => Option<T::BlockNumber>;
		/// The actual future code of a para.
		FutureCode: map hasher(twox_64_concat) ParaId => ValidationCode;

		/// Upcoming paras (chains and threads). These are only updated on session change. Corresponds to an
		/// entry in the upcoming-genesis map.
		UpcomingParas: Vec<ParaId>;
		/// Upcoming paras instantiation arguments.
		UpcomingParasGenesis: map hasher(twox_64_concat) ParaId => Option<ParaGenesisArgs>;
		/// Paras that are to be cleaned up at the end of the session.
		OutgoingParas: Vec<ParaId>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The parachains configuration module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;

	}
}

impl<T: Trait> Module<T> {
	/// Called by the initializer to initialize the configuration module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the configuration module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(_validators: &[ValidatorId], _queued: &[ValidatorId]) {
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn para_past_code_pruning_works_correctly() {
		let mut past_code = ParaPastCodeMeta::default();
		past_code.note_replacement(10u32);
		past_code.note_replacement(20);
		past_code.note_replacement(30);

		let old = past_code.clone();
		assert!(past_code.prune_up_to(9).collect::<Vec<_>>().is_empty());
		assert_eq!(old, past_code);

		assert_eq!(past_code.prune_up_to(10).collect::<Vec<_>>(), vec![10]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![30, 20],
			last_pruned: Some(10),
		});

		assert_eq!(past_code.prune_up_to(21).collect::<Vec<_>>(), vec![20]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![30],
			last_pruned: Some(20),
		});

		past_code.note_replacement(40);
		past_code.note_replacement(50);
		past_code.note_replacement(60);

		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![60, 50, 40, 30],
			last_pruned: Some(20),
		});

		assert_eq!(past_code.prune_up_to(60).collect::<Vec<_>>(), vec![30, 40, 50, 60]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: Vec::new(),
			last_pruned: Some(60),
		});
	}
}
