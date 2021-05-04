// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Runtime component for handling disputes of parachain candidates.

use sp_std::prelude::*;
use sp_std::result;
#[cfg(feature = "std")]
use sp_std::marker::PhantomData;
use primitives::v1::{
	Id as ParaId, ValidationCode, HeadData, SessionIndex, Hash, BlockNumber, CandidateHash,
	DisputeState, DisputeStatementSet, MultiDisputeStatementSet,
};
use sp_runtime::{traits::{One, Saturating}, DispatchResult, DispatchError, SaturatedConversion};
use frame_system::ensure_root;
use frame_support::{
	decl_storage, decl_module, decl_error, decl_event, ensure,
	traits::Get,
	weights::Weight,
};
use parity_scale_codec::{Encode, Decode};
use crate::{
	configuration::{self, HostConfiguration},
	shared,
	initializer::SessionChangeNotification,
};
use sp_core::RuntimeDebug;

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

pub trait Config:
	frame_system::Config +
	configuration::Config
{
	type Event: From<Event> + Into<<Self as frame_system::Config>::Event>;
}

decl_storage! {
	trait Store for Module<T: Config> as ParaDisputes {
		/// The last pruneed session, if any. All data stored by this module
		/// references sessions.
		LastPrunedSession: Option<SessionIndex>;
		// All ongoing or concluded disputes for the last several sessions.
		Disputes: double_map
			hasher(twox_64_concat) SessionIndex,
			hasher(blake2_128_concat) CandidateHash
			=> Option<DisputeState<T::BlockNumber>>;
		// All included blocks on the chain, as well as the block number in this chain that
		// should be reverted back to if the candidate is disputed and determined to be invalid.
		Included: double_map
			hasher(twox_64_concat) SessionIndex,
			hasher(blake2_128_concat) CandidateHash
			=> Option<T::BlockNumber>;
		// Maps session indices to a vector indicating the number of potentially-spam disputes
		// each validator is participating in. Potentially-spam disputes are remote disputes which have
		// fewer than `byzantine_threshold + 1` validators.
		//
		// The i'th entry of the vector corresponds to the i'th validator in the session.
		SpamSlots: map hasher(twox_64_concat) SessionIndex => Vec<u32>;
		// Whether the chain is frozen or not. Starts as `false`. When this is `true`,
		// the chain will not accept any new parachain blocks for backing or inclusion.
		// It can only be set back to `false` by governance intervention.
		Frozen: bool;
	}
}

decl_event! {
	pub enum Event {
		/// A dispute has been initiated. The boolean is true if the dispute is local. \[para_id\]
		DisputeInitiated(ParaId, CandidateHash, bool),
		/// A dispute has concluded for or against a candidate. The boolean is true if the candidate
		/// is deemed valid (for) and false if invalid (against).
		DisputeConcluded(ParaId, CandidateHash, bool),
		/// A dispute has timed out due to insufficient participation.
		DisputeTimedOut(ParaId, CandidateHash),
	}
}

decl_module! {
	/// The parachains configuration module.
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		fn deposit_event() = default;
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// Duplicate dispute statement sets provided.
		DuplicateDisputeStatementSets,
		/// Ancient dispute statement provided.
		AncientDisputeStatement,
	}
}

impl<T: Config> Module<T> {
	/// Called by the iniitalizer to initialize the disputes module.
	pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
		let config = <configuration::Module<T>>::config();

		let mut weight = 0;
		for (session_index, candidate_hash, mut dispute) in <Disputes<T>>::iter() {
			weight += T::DbWeight::get().reads_writes(1, 0);

			if dispute.concluded_at.is_none()
				&& dispute.start + config.dispute_conclusion_by_time_out_period < now
			{
				dispute.concluded_at = Some(now);
				<Disputes<T>>::insert(session_index, candidate_hash, &dispute);

				// mildly punish all validators involved. they've failed to make
				// data available to others, so this is most likely spam.
				SpamSlots::mutate(session_index, |spam_slots| {
					let participating = (dispute.validators_for | dispute.validators_against);
					for validator_index in participating {
						// TODO [now]: slight punishment.

						// also reduce spam slots for all validators involved. this does
						// open us up to more spam, but only for validators who are willing
						// to be punished more.
						if let Some(occupied) = spam_slots.get_mut(validator_index as usize) {
							*occupied = occupied.saturating_sub(1);
						}
					}
				});

				weight += T::DbWeight::get().reads_writes(1, 2);
			}
		}

		weight
	}

	/// Called by the iniitalizer to finalize the disputes module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the iniitalizer to note a new session in the disputes module.
	pub(crate) fn initializer_on_new_session(notification: &SessionChangeNotification<T::BlockNumber>) {
		let config = <configuration::Pallet<T>>::config();

		if notification.session_index <= config.dispute_period + 1 {
			return
		}

		let pruning_target = notification.session_index - config.dispute_period - 1;

		LastPrunedSession::mutate(|last_pruned| {
			if let Some(last_pruned) = last_pruned {
				for to_prune in *last_pruned + 1 ..= pruning_target {
					<Disputes<T>>::remove_prefix(to_prune);
					<Included<T>>::remove_prefix(to_prune);
					SpamSlots::remove(to_prune);
				}
			}

			*last_pruned = Some(pruning_target);
		});
	}

	/// Handle sets of dispute statements corresponding to 0 or more candidates.
	pub(crate) fn provide_multi_dispute_data(statement_sets: MultiDisputeStatementSet)
		-> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError>
	{
		let config = <configuration::Pallet<T>>::config();

		// Deduplicate.
		{
			let mut targets: Vec<_> = statement_sets.iter()
				.map(|set| (set.candidate_hash.0, set.session))
				.collect();

			targets.sort();

			let submitted = targets.len();
			targets.dedup();

			ensure!(submitted == targets.len(), Error::<T>::DuplicateDisputeStatementSets);
		}

		let mut fresh = Vec::with_capacity(statement_sets.len());
		for statement_set in statement_sets {
			let dispute_target = (statement_set.session, statement_set.candidate_hash);
			if Self::provide_dispute_data(&config, statement_set)? {
				fresh.push(dispute_target);
			}
		}

		Ok(fresh)
	}

	/// Handle a set of dispute statements corresponding to a single candidate.
	///
	/// Fails if the dispute data is invalid. Returns a bool indicating whether the
	/// dispute is fresh.
	fn provide_dispute_data(config: &HostConfiguration<T::BlockNumber>, set: DisputeStatementSet)
		-> Result<bool, DispatchError>
	{
		// Dispute statement sets on any dispute which concluded
		// before this point are to be rejected.
		let oldest_accepted = <frame_system::Pallet<T>>::block_number()
			.saturating_sub(config.dispute_post_conclusion_acceptance_period);

		// Check for ancient.
		let fresh = if let Some(dispute_state) = <Disputes<T>>::get(&set.session, &set.candidate_hash) {
			ensure!(
				dispute_state.concluded_at.as_ref().map_or(true, |c| c >= &oldest_accepted),
				Error::<T>::AncientDisputeStatement,
			);

			false
		} else {
			true
		};

		unimplemented!()
		// TODO [now]

		fresh
	}
}
