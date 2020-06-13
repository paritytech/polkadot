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
	traits::Get,
};
use codec::{Encode, Decode};
use system::ensure_root;
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
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
pub struct CandidatePendingAvailability<H, N> {
	/// The availability core this is assigned to.
	core: CoreIndex,
	/// The candidate receipt itself.
	receipt: AbridgedCandidateReceipt<H>,
	/// The received availability votes. One bit per validator.
	availability_votes: BitVec<BitOrderLsb0, u8>,
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
			=> Option<CandidatePendingAvailability<T::Hash, T::BlockNumber>>;

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
		/// The bitfield contains a bit relating to an unassigned availability core.
		UnoccupiedBitInBitfield,
		/// Invalid group index in core assignment.
		InvalidGroupIndex,
		/// Insufficient (non-majority) backing.
		InsufficientBacking,
		/// Invalid (bad signature, unknown validator, etc.) backing.
		InvalidBacking,
	}
}

decl_module! {
	/// The parachain-candidate inclusion module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {

	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight { 0 }

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() { }

	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>
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

		let n_bits = parachains.len() + config.parathread_cores as usize;

		let mut assigned_paras_record: Vec<_> = (0..n_bits)
			.map(|bit_index| core_lookup(CoreIndex::from(bit_index as u32)))
			.map(|core_para| core_para.map(|p| (p, PendingAvailability::<T>::get(&p))))
			.collect();

		// do sanity checks on the bitfields:
		// 1. no more than one bitfield per validator
		// 2. bitfields are ascending by validator index.
		// 3. each bitfield has exactly `n_bits`
		// 4. signature is valid.
		{
			let occupied_bitmask: BitVec<BitOrderLsb0, u8> = assigned_paras_record.iter()
				.map(|p| p.is_some())
				.collect();

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

				ensure!(
					occupied_bitmask.clone() & signed_bitfield.bitfield.0.clone() == signed_bitfield.bitfield.0,
					Error::<T>::UnoccupiedBitInBitfield,
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

		let now = <system::Module<T>>::block_number();
		for signed_bitfield in signed_bitfields.0 {
			for (bit_idx, _)
				in signed_bitfield.bitfield.0.iter().enumerate().filter(|(_, is_av)| **is_av)
			{
				let record = assigned_paras_record[bit_idx]
					.as_mut()
					.expect("validator bitfields checked not to contain bits corresponding to unoccupied cores; qed");

				// defensive check - this is constructed by loading the availability bitfield record,
				// which is always `Some` if the core is occupied - that's why we're here.
				let val_idx = signed_bitfield.validator_index as usize;
				if let Some(mut bit) = record.1.as_mut()
					.and_then(|r| r.availability_votes.get_mut(val_idx))
				{
					*bit = true;
				}
			}

			let record = AvailabilityBitfieldRecord {
				bitfield: signed_bitfield.bitfield,
				submitted_at: now,
			};

			<AvailabilityBitfields<T>>::insert(&signed_bitfield.validator_index, record);
		}

		let threshold = {
			let mut threshold = (validators.len() * 2) / 3;
			threshold += (validators.len() * 2) % 3;
			threshold
		};

		let mut freed_cores = Vec::with_capacity(n_bits);
		for (para_id, pending_availability) in assigned_paras_record.into_iter()
			.filter_map(|x| x)
			.filter_map(|(id, p)| p.map(|p| (id, p)))
		{
			if pending_availability.availability_votes.count_ones() >= threshold {
				<PendingAvailability<T>>::remove(&para_id);
				Self::enact_candidate(
					pending_availability.relay_parent_number,
					pending_availability.receipt,
				);

				freed_cores.push(pending_availability.core);
			} else {
				<PendingAvailability<T>>::insert(&para_id, &pending_availability);
			}
		}

		// TODO: pass available candidates onwards to validity module once implemented.
		// https://github.com/paritytech/polkadot/issues/1251

		Ok(freed_cores)
	}

	/// Process candidates that have been backed. Provide a set of candidates and scheduled cores.
	///
	/// Both should be sorted ascending by core index, and the candidates should be a subset of
	/// scheduled cores. If these conditions are not met, the execution of the function fails.
	pub(crate) fn process_candidates(
		candidates: Vec<BackedCandidate<T::Hash>>,
		scheduled: Vec<CoreAssignment>,
		group_validators: impl Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>,
	)
		-> Result<Vec<CoreIndex>, DispatchError>
	{
		ensure!(candidates.len() <= scheduled.len(), Error::<T>::UnscheduledCandidate);

		if scheduled.is_empty() {
			return Ok(Vec::new());
		}

		let validators = Validators::get();
		let parent_hash = <system::Module<T>>::parent_hash();
		let config = <configuration::Module<T>>::config();
		let now = <system::Module<T>>::block_number();
		let relay_parent_number = now - One::one();

		// do all checks before writing storage.
		let core_indices = {
			let mut skip = 0;
			let mut core_indices = Vec::with_capacity(candidates.len());
			let mut last_core = None;

			let mut check_assignment_in_order = |assignment: &CoreAssignment| -> DispatchResult {
				ensure!(
					last_core.map_or(true, |core| assignment.core > core),
					Error::<T>::ScheduledOutOfOrder,
				);

				last_core = Some(assignment.core);
				Ok(())
			};

			let signing_context = SigningContext {
				parent_hash,
				session_index: CurrentSessionIndex::get(),
			};

			// We combine an outer loop over candidates with an inner loop over the scheduled,
			// where each iteration of the outer loop picks up at the position
			// in scheduled just after the past iteration left off.
			//
			// If the candidates appear in the same order as they appear in `scheduled`,
			// then they should always be found. If the end of `scheduled` is reached,
			// then the candidate was either not scheduled or out-of-order.
			//
			// In the meantime, we do certain sanity checks on the candidates and on the scheduled
			// list.
			'a:
			for candidate in &candidates {
				let para_id = candidate.candidate.parachain_index;

				// we require that the candidate is in the context of the parent block.
				ensure!(
					candidate.candidate.relay_parent == parent_hash,
					Error::<T>::CandidateNotInParentContext,
				);

				let code_upgrade_allowed = <paras::Module<T>>::last_code_upgrade(para_id, true)
					.map_or(
						true,
						|last| last <= relay_parent_number &&
							relay_parent_number.saturating_sub(last) >= config.validation_upgrade_frequency,
					);

				ensure!(code_upgrade_allowed, Error::<T>::PrematureCodeUpgrade);
				for (i, assignment) in scheduled[skip..].iter().enumerate() {
					check_assignment_in_order(assignment)?;

					if candidate.candidate.parachain_index == assignment.para_id {
						if let Some(required_collator) = assignment.required_collator() {
							ensure!(
								required_collator == &candidate.candidate.collator,
								Error::<T>::WrongCollator,
							);
						}

						ensure!(
							<PendingAvailability<T>>::get(&assignment.para_id).is_none(),
							Error::<T>::CandidateScheduledBeforeParaFree,
						);

						// account for already skipped, and then skip this one.
						skip = i + skip + 1;

						let group_vals = group_validators(assignment.group_idx)
							.ok_or_else(|| Error::<T>::InvalidGroupIndex)?;

						// check the signatures in the backing and that it is a majority.
						{
							let maybe_amount_validated
								= primitives::parachain::check_candidate_backing(
									&candidate,
									&signing_context,
									group_vals.len(),
									|idx| group_vals.get(idx)
										.and_then(|i| validators.get(*i as usize))
										.map(|v| v.clone()),
								);

							match maybe_amount_validated {
								Ok(amount_validated) => ensure!(
									amount_validated * 2 > group_vals.len(),
									Error::<T>::InsufficientBacking,
								),
								Err(()) => { Err(Error::<T>::InvalidBacking)?; }
							}
						}

						core_indices.push(assignment.core);
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
			};

			// check remainder of scheduled cores, if any.
			for assignment in scheduled[skip..].iter() {
				check_assignment_in_order(assignment)?;
			}

			core_indices
		};

		// one more sweep for actually writing to storage.
		for (candidate, core) in candidates.into_iter().zip(core_indices.iter().cloned()) {
			let para_id = candidate.candidate.parachain_index;

			// initialize all availability votes to 0.
			let availability_votes: BitVec<BitOrderLsb0, u8>
				= bitvec::bitvec![BitOrderLsb0, u8; 0; validators.len()];
			<PendingAvailability<T>>::insert(&para_id, CandidatePendingAvailability {
				core,
				receipt: candidate.candidate,
				availability_votes,
				relay_parent_number,
				backed_in_number: now,
			});
		}

		Ok(core_indices)
	}

	fn enact_candidate(
		relay_parent_number: T::BlockNumber,
		receipt: AbridgedCandidateReceipt<T::Hash>,
	) -> Weight {
		let commitments = receipt.commitments;
		let config = <configuration::Module<T>>::config();

		// initial weight is config read.
		let mut weight = T::DbWeight::get().reads_writes(1, 0);
		if let Some(new_code) = commitments.new_validation_code {
			weight += <paras::Module<T>>::schedule_code_upgrade(
				receipt.parachain_index,
				new_code,
				relay_parent_number + config.validation_upgrade_delay,
			);
		}

		weight + <paras::Module<T>>::note_new_head(
			receipt.parachain_index,
			receipt.head_data,
			relay_parent_number,
		)
	}
}
