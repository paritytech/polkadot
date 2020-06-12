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

//! The inclusion module is responsible for inclusion and availability of scheduled parachains
//! and parathreads.
//!
//! It is responsible for carrying candidates from being backable to being backed, and then from backed
//! to included.

use sp_std::prelude::*;
use primitives::{
	parachain::{
		ValidatorId, AbridgedCandidateReceipt, ValidatorIndex, Id as ParaId,
		AvailabilityBitfield as AvailabilityBitfield, SignedAvailabilityBitfields, SigningContext,
		BackedCandidate,
	},
};
use frame_support::{
	decl_storage, decl_module, decl_error, ensure, dispatch::DispatchResult, IterableStorageMap,
	weights::{DispatchClass, Weight},
};
use codec::{Encode, Decode};
use system::ensure_root;
use bitvec::vec::BitVec;
use sp_staking::SessionIndex;
use sp_runtime::{DispatchError, traits::{One, Saturating}};

use crate::{configuration, paras, scheduler::{CoreIndex, GroupIndex, CoreAssignment}};

/// A bitfield signed by a validator indicating that it is keeping its piece of the erasure-coding
/// for any backed candidates referred to by a `1` bit available.
#[derive(Encode, Decode)]
#[cfg_attr(test, derive(Debug))]
pub struct AvailabilityBitfieldRecord<N> {
	bitfield: AvailabilityBitfield, // one bit per core.
	submitted_at: N, // for accounting, as meaning of bits may change over time.
}

/// A backed candidate pending availability.
#[derive(Encode, Decode)]
#[cfg_attr(test, derive(Debug))]
pub struct CandidatePendingAvailability<N> {
	/// The availability core this is assigned to.
	core: CoreIndex,
	/// The candidate receipt itself.
	receipt: AbridgedCandidateReceipt,
	/// The received availability votes. One bit per validator.
	availability_votes: bitvec::vec::BitVec<bitvec::order::Lsb0, u8>,
	/// The block number of the relay-parent of the receipt.
	relay_parent_number: N,
	/// The block number of the relay-chain block this was backed in.
	backed_in_number: N,
}

pub trait Trait: system::Trait + paras::Trait + configuration::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as ParaInclusion {
		/// The latest bitfield for each validator, referred to by their index in the validator set.
		AvailabilityBitfields: map hasher(twox_64_concat) ValidatorIndex
			=> Option<AvailabilityBitfieldRecord<T::BlockNumber>>;

		/// Candidates pending availability by `ParaId`.
		PendingAvailability: map hasher(twox_64_concat) ParaId
			=> Option<CandidatePendingAvailability<T::BlockNumber>>;

		/// The current validators, by their parachain session keys.
		Validators get(fn validators) config(validators): Vec<ValidatorId>;

		/// The current session index.
		CurrentSessionIndex: SessionIndex;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Availability bitfield has unexpected size.
		WrongBitfieldSize,
		/// Multiple bitfields submitted by same validator or validators out of order by index.
		BitfieldDuplicateOrUnordered,
		/// Validator index out of bounds.
		ValidatorIndexOutOfBounds,
		/// Invalid signature
		InvalidBitfieldSignature,
		/// Candidate submitted but para not scheduled.
		UnscheduledCandidate,
		/// Candidate scheduled despite pending candidate already existing for the para.
		CandidateScheduledBeforeParaFree,
		/// Candidate included with the wrong collator.
		WrongCollator,
		/// Scheduled cores out of order.
		ScheduledOutOfOrder,
		/// Code upgrade prematurely.
		PrematureCodeUpgrade,
		/// Candidate not in parent context.
		CandidateNotInParentContext,
	}
}

decl_module! {
	/// The parachain-candidate inclusion module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {

	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: crate::initializer::SessionChangeNotification<T::BlockNumber>
	) {
		// unlike most drain methods, drained elements are not cleared on `Drop` of the iterator
		// and require consumption.
		for _ in <PendingAvailability<T>>::drain() { }
		for _ in <AvailabilityBitfields<T>>::drain() { }

		Validators::set(notification.validators.clone()); // substrate forces us to clone, stupidly.
		CurrentSessionIndex::set(notification.session_index);
	}

	/// Process a set of incoming bitfields. Return a vec of cores freed by candidates
	/// becoming available.
	pub(crate) fn process_bitfields(
		signed_bitfields: SignedAvailabilityBitfields,
		core_lookup: impl Fn(CoreIndex) -> Option<ParaId>,
	) -> Result<Vec<CoreIndex>, DispatchError> {
		let validators = Validators::get();
		let session_index = CurrentSessionIndex::get();
		let config = <configuration::Module<T>>::config();
		let parachains = <paras::Module<T>>::parachains();

		let n_validators = validators.len();
		let n_bits = parachains.len() + config.parathread_cores as usize;

		// do sanity checks on the bitfields:
		// 1. no more than one bitfield per validator
		// 2. bitfields are ascending by validator index.
		// 3. each bitfield has exactly `n_bits`
		// 4. signature is valid.
		{
			let mut last_index = None;
			let mut payload_encode_buf = Vec::new();

			let signing_context = SigningContext {
				parent_hash: <system::Module<T>>::parent_hash(),
				session_index,
			};

			for signed_bitfield in &signed_bitfields.0 {
				ensure!(
					signed_bitfield.bitfield.0.len() == n_bits,
					Error::<T>::WrongBitfieldSize,
				);

				ensure!(
					last_index.map_or(true, |last| last < signed_bitfield.validator_index),
					Error::<T>::BitfieldDuplicateOrUnordered,
				);

				ensure!(
					signed_bitfield.validator_index < validators.len() as _,
					Error::<T>::ValidatorIndexOutOfBounds,
				);

				let validator_public = &validators[signed_bitfield.validator_index as usize];

				if let Err(()) = primitives::parachain::check_availability_bitfield_signature(
					&signed_bitfield.bitfield,
					validator_public,
					&signed_bitfield.signature,
					&signing_context,
					Some(&mut payload_encode_buf),
				) {
					Err(Error::<T>::InvalidBitfieldSignature)?;
				}

				last_index = Some(signed_bitfield.validator_index);
				payload_encode_buf.clear();
			}
		}

		// TODO [now] actually apply bitfields

		// TODO: pass available candidates onwards to validity module once implemented.
		// https://github.com/paritytech/polkadot/issues/1251

		Ok(Vec::new())
	}

	/// Process candidates that have been backed. Provide a set of candidates and scheduled cores.
	///
	/// Both should be sorted ascending by core index, and the candidates should be a subset of
	/// scheduled cores. If these conditions are not met, the execution of the function fails.
	pub(crate) fn process_candidates(
		candidates: Vec<BackedCandidate>,
		scheduled: Vec<CoreAssignment>,
		group_validators: impl Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>,
	)
		-> Result<Vec<CoreIndex>, DispatchError>
	{
		ensure!(
			candidates.len() <= scheduled.len(),
			Error::<T>::UnscheduledCandidate,
		);

		if scheduled.is_empty() { return Ok(Vec::new()) }

		let mut indices_in_scheduled = {
			let mut skip = 0;
			let mut indices = Vec::with_capacity(candidates.len());
			let mut last_core = None;

			let mut check_assignment_in_order = |assignment: &CoreAssignment| -> DispatchResult {
				ensure!(
					last_core.map_or(true, |core| assignment.core > core),
					Error::<T>::ScheduledOutOfOrder,
				);

				last_core = Some(assignment.core);
				Ok(())
			};

			'a:
			for candidate in &candidates {
				for (i, assignment) in scheduled[skip..].iter().enumerate() {
					check_assignment_in_order(assignment)?;

					if candidate.candidate.parachain_index == assignment.para_id {
						// account for already skipped.
						let index_in_scheduled = i + skip;
						skip = index_in_scheduled;

						indices.push(index_in_scheduled);
						continue 'a;
					}
				}

				// end of loop reached means that the candidate didn't appear in the non-traversed
				// section of the `scheduled` slice. either it was not scheduled or didn't appear in
				// `candidates` in the correct order.
				ensure!(
					false,
					Error::<T>::UnscheduledCandidate,
				);
			}

			// check remainder of scheduled cores, if any.
			for assignment in scheduled[skip..].iter() {
				check_assignment_in_order(assignment)?;
			}

			indices
		};

		let core_assignments = indices_in_scheduled.iter().cloned().map(|i| &scheduled[i]);
		let config = <configuration::Module<T>>::config();
		let now = <system::Module<T>>::block_number();
		let parent_hash = <system::Module<T>>::parent_hash();
		let validators = Validators::get();

		for (candidate, core_assignment) in candidates.into_iter().zip(core_assignments) {
			let para_id = core_assignment.para_id;

			// we require that the candidate is in the context of the parent block.
			// TODO [now]
			// ensure!(
			// 	candidate.candidate.relay_parent == parent_hash,
			// 	Error::<T>::CandidateNotInParentContext,
			// );

			let relay_parent_number = now - One::one();

			ensure!(
				<PendingAvailability<T>>::get(&core_assignment.para_id).is_none(),
				Error::<T>::CandidateScheduledBeforeParaFree,
			);

			if let Some(required_collator) = core_assignment.required_collator() {
				ensure!(
					required_collator == &candidate.candidate.collator,
					Error::<T>::WrongCollator,
				);
			}

			let code_upgrade_allowed = <paras::Module<T>>::last_code_upgrade(para_id, true).map_or(
				true,
				|last| relay_parent_number.saturating_sub(last)
					>= config.validation_upgrade_frequency,
			);

			ensure!(code_upgrade_allowed, Error::<T>::PrematureCodeUpgrade);

			// TODO [now]: signature check. needs localvalidationdata / globalvalidationschedule?

			// TODO [now]: place into pending candidate storage.
		}

		Ok(Vec::new())
	}
}
