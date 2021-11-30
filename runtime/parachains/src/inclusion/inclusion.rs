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

//! The inclusion pallet is responsible for inclusion and availability of scheduled parachains
//! and parathreads.
//!
//! It is responsible for carrying candidates from being backable to being backed, and then from backed
//! to included.

use crate::{
	configuration, disputes, dmp, hrmp, paras,
	paras_inherent::{sanitize_bitfields, DisputedBitfield},
	scheduler::CoreAssignment,
	shared, ump,
};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_support::pallet_prelude::*;
use parity_scale_codec::{Decode, Encode};
use primitives::v1::{
	AvailabilityBitfield, BackedCandidate, CandidateCommitments, CandidateDescriptor,
	CandidateHash, CandidateReceipt, CommittedCandidateReceipt, CoreIndex, GroupIndex, Hash,
	HeadData, Id as ParaId, SigningContext, UncheckedSignedAvailabilityBitfields, ValidatorId,
	ValidatorIndex, ValidityAttestation,
};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{One, Saturating},
	DispatchError,
};
use sp_std::{collections::btree_set::BTreeSet, prelude::*};

pub use pallet::*;

#[cfg(test)]
pub(crate) mod tests;

/// A bitfield signed by a validator indicating that it is keeping its piece of the erasure-coding
/// for any backed candidates referred to by a `1` bit available.
///
/// The bitfield's signature should be checked at the point of submission. Afterwards it can be
/// dropped.
#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(Debug))]
pub struct AvailabilityBitfieldRecord<N> {
	bitfield: AvailabilityBitfield, // one bit per core.
	submitted_at: N,                // for accounting, as meaning of bits may change over time.
}

/// Determines if all checks should be applied or if a subset was already completed
/// in a code path that will be executed afterwards or was already executed before.
#[derive(Encode, Decode, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub(crate) enum FullCheck {
	/// Yes, do a full check, skip nothing.
	Yes,
	/// Skip a subset of checks that are already completed before.
	///
	/// Attention: Should only be used when absolutely sure that the required
	/// checks are completed before.
	Skip,
}

/// A backed candidate pending availability.
#[derive(Encode, Decode, PartialEq, TypeInfo)]
#[cfg_attr(test, derive(Debug, Default))]
pub struct CandidatePendingAvailability<H, N> {
	/// The availability core this is assigned to.
	core: CoreIndex,
	/// The candidate hash.
	hash: CandidateHash,
	/// The candidate descriptor.
	descriptor: CandidateDescriptor<H>,
	/// The received availability votes. One bit per validator.
	availability_votes: BitVec<BitOrderLsb0, u8>,
	/// The backers of the candidate pending availability.
	backers: BitVec<BitOrderLsb0, u8>,
	/// The block number of the relay-parent of the receipt.
	relay_parent_number: N,
	/// The block number of the relay-chain block this was backed in.
	backed_in_number: N,
	/// The group index backing this block.
	backing_group: GroupIndex,
}

impl<H, N> CandidatePendingAvailability<H, N> {
	/// Get the availability votes on the candidate.
	pub(crate) fn availability_votes(&self) -> &BitVec<BitOrderLsb0, u8> {
		&self.availability_votes
	}

	/// Get the relay-chain block number this was backed in.
	pub(crate) fn backed_in_number(&self) -> &N {
		&self.backed_in_number
	}

	/// Get the core index.
	pub(crate) fn core_occupied(&self) -> CoreIndex {
		self.core.clone()
	}

	/// Get the candidate hash.
	pub(crate) fn candidate_hash(&self) -> CandidateHash {
		self.hash
	}

	/// Get the candidate descriptor.
	pub(crate) fn candidate_descriptor(&self) -> &CandidateDescriptor<H> {
		&self.descriptor
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	pub(crate) fn new(
		core: CoreIndex,
		hash: CandidateHash,
		descriptor: CandidateDescriptor<H>,
		availability_votes: BitVec<BitOrderLsb0, u8>,
		backers: BitVec<BitOrderLsb0, u8>,
		relay_parent_number: N,
		backed_in_number: N,
		backing_group: GroupIndex,
	) -> Self {
		Self {
			core,
			hash,
			descriptor,
			availability_votes,
			backers,
			relay_parent_number,
			backed_in_number,
			backing_group,
		}
	}
}

/// A hook for applying validator rewards
pub trait RewardValidators {
	// Reward the validators with the given indices for issuing backing statements.
	fn reward_backing(validators: impl IntoIterator<Item = ValidatorIndex>);
	// Reward the validators with the given indices for issuing availability bitfields.
	// Validators are sent to this hook when they have contributed to the availability
	// of a candidate by setting a bit in their bitfield.
	fn reward_bitfields(validators: impl IntoIterator<Item = ValidatorIndex>);
}

/// Helper return type for `process_candidates`.
#[derive(Encode, Decode, PartialEq, TypeInfo)]
#[cfg_attr(test, derive(Debug))]
pub(crate) struct ProcessedCandidates<H = Hash> {
	pub(crate) core_indices: Vec<CoreIndex>,
	pub(crate) candidate_receipt_with_backing_validator_indices:
		Vec<(CandidateReceipt<H>, Vec<(ValidatorIndex, ValidityAttestation)>)>,
}

impl<H> Default for ProcessedCandidates<H> {
	fn default() -> Self {
		Self {
			core_indices: Vec::new(),
			candidate_receipt_with_backing_validator_indices: Vec::new(),
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
	pub trait Config:
		frame_system::Config
		+ shared::Config
		+ paras::Config
		+ dmp::Config
		+ ump::Config
		+ hrmp::Config
		+ configuration::Config
	{
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
		type DisputesHandler: disputes::DisputesHandler<Self::BlockNumber>;
		type RewardValidators: RewardValidators;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A candidate was backed. `[candidate, head_data]`
		CandidateBacked(CandidateReceipt<T::Hash>, HeadData, CoreIndex, GroupIndex),
		/// A candidate was included. `[candidate, head_data]`
		CandidateIncluded(CandidateReceipt<T::Hash>, HeadData, CoreIndex, GroupIndex),
		/// A candidate timed out. `[candidate, head_data]`
		CandidateTimedOut(CandidateReceipt<T::Hash>, HeadData, CoreIndex),
	}

	#[pallet::error]
	pub enum Error<T> {
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
		/// Head data exceeds the configured maximum.
		HeadDataTooLarge,
		/// Code upgrade prematurely.
		PrematureCodeUpgrade,
		/// Output code is too large
		NewCodeTooLarge,
		/// Candidate not in parent context.
		CandidateNotInParentContext,
		/// Invalid group index in core assignment.
		InvalidGroupIndex,
		/// Insufficient (non-majority) backing.
		InsufficientBacking,
		/// Invalid (bad signature, unknown validator, etc.) backing.
		InvalidBacking,
		/// Collator did not sign PoV.
		NotCollatorSigned,
		/// The validation data hash does not match expected.
		ValidationDataHashMismatch,
		/// The downward message queue is not processed correctly.
		IncorrectDownwardMessageHandling,
		/// At least one upward message sent does not pass the acceptance criteria.
		InvalidUpwardMessages,
		/// The candidate didn't follow the rules of HRMP watermark advancement.
		HrmpWatermarkMishandling,
		/// The HRMP messages sent by the candidate is not valid.
		InvalidOutboundHrmp,
		/// The validation code hash of the candidate is not valid.
		InvalidValidationCodeHash,
		/// The `para_head` hash in the candidate descriptor doesn't match the hash of the actual para head in the
		/// commitments.
		ParaHeadMismatch,
		/// A bitfield that references a freed core,
		/// either intentionally or as part of a concluded
		/// invalid dispute.
		BitfieldReferencesFreedCore,
	}

	/// The latest bitfield for each validator, referred to by their index in the validator set.
	#[pallet::storage]
	pub(crate) type AvailabilityBitfields<T: Config> =
		StorageMap<_, Twox64Concat, ValidatorIndex, AvailabilityBitfieldRecord<T::BlockNumber>>;

	/// Candidates pending availability by `ParaId`.
	#[pallet::storage]
	pub(crate) type PendingAvailability<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, CandidatePendingAvailability<T::Hash, T::BlockNumber>>;

	/// The commitments of candidates pending availability, by `ParaId`.
	#[pallet::storage]
	pub(crate) type PendingAvailabilityCommitments<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, CandidateCommitments>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

const LOG_TARGET: &str = "runtime::inclusion";

impl<T: Config> Pallet<T> {
	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() {}

	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		_notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>,
	) {
		// unlike most drain methods, drained elements are not cleared on `Drop` of the iterator
		// and require consumption.
		for _ in <PendingAvailabilityCommitments<T>>::drain() {}
		for _ in <PendingAvailability<T>>::drain() {}
		for _ in <AvailabilityBitfields<T>>::drain() {}
	}

	/// Extract the freed cores based on cores that became available.
	///
	/// Updates storage items `PendingAvailability` and `AvailabilityBitfields`.
	pub(crate) fn update_pending_availability_and_get_freed_cores<F, const ON_CHAIN_USE: bool>(
		expected_bits: usize,
		validators: &[ValidatorId],
		signed_bitfields: UncheckedSignedAvailabilityBitfields,
		core_lookup: F,
	) -> Vec<(CoreIndex, CandidateHash)>
	where
		F: Fn(CoreIndex) -> Option<ParaId>,
	{
		let mut assigned_paras_record = (0..expected_bits)
			.map(|bit_index| core_lookup(CoreIndex::from(bit_index as u32)))
			.map(|opt_para_id| {
				opt_para_id.map(|para_id| (para_id, PendingAvailability::<T>::get(&para_id)))
			})
			.collect::<Vec<_>>();

		let now = <frame_system::Pallet<T>>::block_number();
		for (checked_bitfield, validator_index) in
			signed_bitfields.into_iter().map(|signed_bitfield| {
				// extracting unchecked data, since it's checked in `fn sanitize_bitfields` already.
				let validator_idx = signed_bitfield.unchecked_validator_index();
				let checked_bitfield = signed_bitfield.unchecked_into_payload();
				(checked_bitfield, validator_idx)
			}) {
			for (bit_idx, _) in checked_bitfield.0.iter().enumerate().filter(|(_, is_av)| **is_av) {
				let pending_availability = if let Some((_, pending_availability)) =
					assigned_paras_record[bit_idx].as_mut()
				{
					pending_availability
				} else {
					// For honest validators, this happens in case of unoccupied cores,
					// which in turn happens in case of a disputed candidate.
					// A malicious one might include arbitrary indices, but they are represented
					// by `None` values and will be sorted out in the next if case.
					continue
				};

				// defensive check - this is constructed by loading the availability bitfield record,
				// which is always `Some` if the core is occupied - that's why we're here.
				let validator_index = validator_index.0 as usize;
				if let Some(mut bit) =
					pending_availability.as_mut().and_then(|candidate_pending_availability| {
						candidate_pending_availability.availability_votes.get_mut(validator_index)
					}) {
					*bit = true;
				}
			}

			let record =
				AvailabilityBitfieldRecord { bitfield: checked_bitfield, submitted_at: now };

			<AvailabilityBitfields<T>>::insert(&validator_index, record);
		}

		let threshold = availability_threshold(validators.len());

		let mut freed_cores = Vec::with_capacity(expected_bits);
		for (para_id, pending_availability) in assigned_paras_record
			.into_iter()
			.filter_map(|x| x)
			.filter_map(|(id, p)| p.map(|p| (id, p)))
		{
			if pending_availability.availability_votes.count_ones() >= threshold {
				<PendingAvailability<T>>::remove(&para_id);
				let commitments = match PendingAvailabilityCommitments::<T>::take(&para_id) {
					Some(commitments) => commitments,
					None => {
						log::warn!(
							target: LOG_TARGET,
							"Inclusion::process_bitfields: PendingAvailability and PendingAvailabilityCommitments
							are out of sync, did someone mess with the storage?",
						);
						continue
					},
				};

				if ON_CHAIN_USE {
					let receipt = CommittedCandidateReceipt {
						descriptor: pending_availability.descriptor,
						commitments,
					};
					let _weight = Self::enact_candidate(
						pending_availability.relay_parent_number,
						receipt,
						pending_availability.backers,
						pending_availability.availability_votes,
						pending_availability.core,
						pending_availability.backing_group,
					);
				}

				freed_cores.push((pending_availability.core, pending_availability.hash));
			} else {
				<PendingAvailability<T>>::insert(&para_id, &pending_availability);
			}
		}

		freed_cores
	}

	/// Process a set of incoming bitfields.
	///
	/// Returns a `Vec` of `CandidateHash`es and their respective `AvailabilityCore`s that became available,
	/// and cores free.
	pub(crate) fn process_bitfields(
		expected_bits: usize,
		signed_bitfields: UncheckedSignedAvailabilityBitfields,
		disputed_bitfield: DisputedBitfield,
		core_lookup: impl Fn(CoreIndex) -> Option<ParaId>,
	) -> Vec<(CoreIndex, CandidateHash)> {
		let validators = shared::Pallet::<T>::active_validator_keys();
		let session_index = shared::Pallet::<T>::session_index();
		let parent_hash = frame_system::Pallet::<T>::parent_hash();

		let checked_bitfields = sanitize_bitfields::<T>(
			signed_bitfields,
			disputed_bitfield,
			expected_bits,
			parent_hash,
			session_index,
			&validators[..],
			FullCheck::Yes,
		);

		let freed_cores = Self::update_pending_availability_and_get_freed_cores::<_, true>(
			expected_bits,
			&validators[..],
			checked_bitfields,
			core_lookup,
		);

		freed_cores
	}

	/// Process candidates that have been backed. Provide the relay storage root, a set of candidates
	/// and scheduled cores.
	///
	/// Both should be sorted ascending by core index, and the candidates should be a subset of
	/// scheduled cores. If these conditions are not met, the execution of the function fails.
	pub(crate) fn process_candidates<GV>(
		parent_storage_root: T::Hash,
		candidates: Vec<BackedCandidate<T::Hash>>,
		scheduled: Vec<CoreAssignment>,
		group_validators: GV,
		full_check: FullCheck,
	) -> Result<ProcessedCandidates<T::Hash>, DispatchError>
	where
		GV: Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>,
	{
		ensure!(candidates.len() <= scheduled.len(), Error::<T>::UnscheduledCandidate);

		if scheduled.is_empty() {
			return Ok(ProcessedCandidates::default())
		}

		let validators = shared::Pallet::<T>::active_validator_keys();
		let parent_hash = <frame_system::Pallet<T>>::parent_hash();

		// At the moment we assume (and in fact enforce, below) that the relay-parent is always one
		// before of the block where we include a candidate (i.e. this code path).
		let now = <frame_system::Pallet<T>>::block_number();
		let relay_parent_number = now - One::one();
		let check_ctx = CandidateCheckContext::<T>::new(now, relay_parent_number);

		// Collect candidate receipts with backers.
		let mut candidate_receipt_with_backing_validator_indices =
			Vec::with_capacity(candidates.len());

		// Do all checks before writing storage.
		let core_indices_and_backers = {
			let mut skip = 0;
			let mut core_indices_and_backers = Vec::with_capacity(candidates.len());
			let mut last_core = None;

			let mut check_assignment_in_order = |assignment: &CoreAssignment| -> DispatchResult {
				ensure!(
					last_core.map_or(true, |core| assignment.core > core),
					Error::<T>::ScheduledOutOfOrder,
				);

				last_core = Some(assignment.core);
				Ok(())
			};

			let signing_context =
				SigningContext { parent_hash, session_index: shared::Pallet::<T>::session_index() };

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
			'next_backed_candidate: for (candidate_idx, backed_candidate) in
				candidates.iter().enumerate()
			{
				if let FullCheck::Yes = full_check {
					check_ctx.verify_backed_candidate(
						parent_hash,
						candidate_idx,
						backed_candidate,
					)?;
				}

				let para_id = backed_candidate.descriptor().para_id;
				let mut backers = bitvec::bitvec![BitOrderLsb0, u8; 0; validators.len()];

				for (i, assignment) in scheduled[skip..].iter().enumerate() {
					check_assignment_in_order(assignment)?;

					if para_id == assignment.para_id {
						if let Some(required_collator) = assignment.required_collator() {
							ensure!(
								required_collator == &backed_candidate.descriptor().collator,
								Error::<T>::WrongCollator,
							);
						}

						{
							// this should never fail because the para is registered
							let persisted_validation_data =
								match crate::util::make_persisted_validation_data::<T>(
									para_id,
									relay_parent_number,
									parent_storage_root,
								) {
									Some(l) => l,
									None => {
										// We don't want to error out here because it will
										// brick the relay-chain. So we return early without
										// doing anything.
										return Ok(ProcessedCandidates::default())
									},
								};

							let expected = persisted_validation_data.hash();

							ensure!(
								expected ==
									backed_candidate.descriptor().persisted_validation_data_hash,
								Error::<T>::ValidationDataHashMismatch,
							);
						}

						ensure!(
							<PendingAvailability<T>>::get(&para_id).is_none() &&
								<PendingAvailabilityCommitments<T>>::get(&para_id).is_none(),
							Error::<T>::CandidateScheduledBeforeParaFree,
						);

						// account for already skipped, and then skip this one.
						skip = i + skip + 1;

						let group_vals = group_validators(assignment.group_idx)
							.ok_or_else(|| Error::<T>::InvalidGroupIndex)?;

						// check the signatures in the backing and that it is a majority.
						{
							let maybe_amount_validated = primitives::v1::check_candidate_backing(
								&backed_candidate,
								&signing_context,
								group_vals.len(),
								|intra_group_vi| {
									group_vals
										.get(intra_group_vi)
										.and_then(|vi| validators.get(vi.0 as usize))
										.map(|v| v.clone())
								},
							);

							match maybe_amount_validated {
								Ok(amount_validated) => ensure!(
									amount_validated * 2 > group_vals.len(),
									Error::<T>::InsufficientBacking,
								),
								Err(()) => {
									Err(Error::<T>::InvalidBacking)?;
								},
							}

							let mut backer_idx_and_attestation =
								Vec::<(ValidatorIndex, ValidityAttestation)>::with_capacity(
									backed_candidate.validator_indices.count_ones(),
								);
							let candidate_receipt = backed_candidate.receipt();

							for ((bit_idx, _), attestation) in backed_candidate
								.validator_indices
								.iter()
								.enumerate()
								.filter(|(_, signed)| **signed)
								.zip(backed_candidate.validity_votes.iter().cloned())
							{
								let val_idx = group_vals
									.get(bit_idx)
									.expect("this query succeeded above; qed");
								backer_idx_and_attestation.push((*val_idx, attestation));

								backers.set(val_idx.0 as _, true);
							}
							candidate_receipt_with_backing_validator_indices
								.push((candidate_receipt, backer_idx_and_attestation));
						}

						core_indices_and_backers.push((
							assignment.core,
							backers,
							assignment.group_idx,
						));
						continue 'next_backed_candidate
					}
				}

				// end of loop reached means that the candidate didn't appear in the non-traversed
				// section of the `scheduled` slice. either it was not scheduled or didn't appear in
				// `candidates` in the correct order.
				ensure!(false, Error::<T>::UnscheduledCandidate);
			}

			// check remainder of scheduled cores, if any.
			for assignment in scheduled[skip..].iter() {
				check_assignment_in_order(assignment)?;
			}

			core_indices_and_backers
		};

		// one more sweep for actually writing to storage.
		let core_indices =
			core_indices_and_backers.iter().map(|&(ref c, _, _)| c.clone()).collect();
		for (candidate, (core, backers, group)) in
			candidates.into_iter().zip(core_indices_and_backers)
		{
			let para_id = candidate.descriptor().para_id;

			// initialize all availability votes to 0.
			let availability_votes: BitVec<BitOrderLsb0, u8> =
				bitvec::bitvec![BitOrderLsb0, u8; 0; validators.len()];

			Self::deposit_event(Event::<T>::CandidateBacked(
				candidate.candidate.to_plain(),
				candidate.candidate.commitments.head_data.clone(),
				core,
				group,
			));

			let candidate_hash = candidate.candidate.hash();

			let (descriptor, commitments) =
				(candidate.candidate.descriptor, candidate.candidate.commitments);

			<PendingAvailability<T>>::insert(
				&para_id,
				CandidatePendingAvailability {
					core,
					hash: candidate_hash,
					descriptor,
					availability_votes,
					relay_parent_number,
					backers: backers.to_bitvec(),
					backed_in_number: check_ctx.now,
					backing_group: group,
				},
			);
			<PendingAvailabilityCommitments<T>>::insert(&para_id, commitments);
		}

		Ok(ProcessedCandidates::<T::Hash> {
			core_indices,
			candidate_receipt_with_backing_validator_indices,
		})
	}

	/// Run the acceptance criteria checks on the given candidate commitments.
	pub(crate) fn check_validation_outputs_for_runtime_api(
		para_id: ParaId,
		validation_outputs: primitives::v1::CandidateCommitments,
	) -> bool {
		// This function is meant to be called from the runtime APIs against the relay-parent, hence
		// `relay_parent_number` is equal to `now`.
		let now = <frame_system::Pallet<T>>::block_number();
		let relay_parent_number = now;
		let check_ctx = CandidateCheckContext::<T>::new(now, relay_parent_number);

		if let Err(err) = check_ctx.check_validation_outputs(
			para_id,
			&validation_outputs.head_data,
			&validation_outputs.new_validation_code,
			validation_outputs.processed_downward_messages,
			&validation_outputs.upward_messages,
			T::BlockNumber::from(validation_outputs.hrmp_watermark),
			&validation_outputs.horizontal_messages,
		) {
			log::debug!(
				target: LOG_TARGET,
				"Validation outputs checking for parachain `{}` failed: {:?}",
				u32::from(para_id),
				err,
			);
			false
		} else {
			true
		}
	}

	fn enact_candidate(
		relay_parent_number: T::BlockNumber,
		receipt: CommittedCandidateReceipt<T::Hash>,
		backers: BitVec<BitOrderLsb0, u8>,
		availability_votes: BitVec<BitOrderLsb0, u8>,
		core_index: CoreIndex,
		backing_group: GroupIndex,
	) -> Weight {
		let plain = receipt.to_plain();
		let commitments = receipt.commitments;
		let config = <configuration::Pallet<T>>::config();

		T::RewardValidators::reward_backing(
			backers
				.iter()
				.enumerate()
				.filter(|(_, backed)| **backed)
				.map(|(i, _)| ValidatorIndex(i as _)),
		);

		T::RewardValidators::reward_bitfields(
			availability_votes
				.iter()
				.enumerate()
				.filter(|(_, voted)| **voted)
				.map(|(i, _)| ValidatorIndex(i as _)),
		);

		// initial weight is config read.
		let mut weight = T::DbWeight::get().reads_writes(1, 0);
		if let Some(new_code) = commitments.new_validation_code {
			weight += <paras::Pallet<T>>::schedule_code_upgrade(
				receipt.descriptor.para_id,
				new_code,
				relay_parent_number,
				&config,
			);
		}

		// enact the messaging facet of the candidate.
		weight += <dmp::Pallet<T>>::prune_dmq(
			receipt.descriptor.para_id,
			commitments.processed_downward_messages,
		);
		weight += <ump::Pallet<T>>::receive_upward_messages(
			receipt.descriptor.para_id,
			commitments.upward_messages,
		);
		weight += <hrmp::Pallet<T>>::prune_hrmp(
			receipt.descriptor.para_id,
			T::BlockNumber::from(commitments.hrmp_watermark),
		);
		weight += <hrmp::Pallet<T>>::queue_outbound_hrmp(
			receipt.descriptor.para_id,
			commitments.horizontal_messages,
		);

		Self::deposit_event(Event::<T>::CandidateIncluded(
			plain,
			commitments.head_data.clone(),
			core_index,
			backing_group,
		));

		weight +
			<paras::Pallet<T>>::note_new_head(
				receipt.descriptor.para_id,
				commitments.head_data,
				relay_parent_number,
			)
	}

	/// Cleans up all paras pending availability that the predicate returns true for.
	///
	/// The predicate accepts the index of the core and the block number the core has been occupied
	/// since (i.e. the block number the candidate was backed at in this fork of the relay chain).
	///
	/// Returns a vector of cleaned-up core IDs.
	pub(crate) fn collect_pending(
		pred: impl Fn(CoreIndex, T::BlockNumber) -> bool,
	) -> Vec<CoreIndex> {
		let mut cleaned_up_ids = Vec::new();
		let mut cleaned_up_cores = Vec::new();

		for (para_id, pending_record) in <PendingAvailability<T>>::iter() {
			if pred(pending_record.core, pending_record.backed_in_number) {
				cleaned_up_ids.push(para_id);
				cleaned_up_cores.push(pending_record.core);
			}
		}

		for para_id in cleaned_up_ids {
			let pending = <PendingAvailability<T>>::take(&para_id);
			let commitments = <PendingAvailabilityCommitments<T>>::take(&para_id);

			if let (Some(pending), Some(commitments)) = (pending, commitments) {
				// defensive: this should always be true.
				let candidate = CandidateReceipt {
					descriptor: pending.descriptor,
					commitments_hash: commitments.hash(),
				};

				Self::deposit_event(Event::<T>::CandidateTimedOut(
					candidate,
					commitments.head_data,
					pending.core,
				));
			}
		}

		cleaned_up_cores
	}

	/// Cleans up all paras pending availability that are in the given list of disputed candidates.
	///
	/// Returns a vector of cleaned-up core IDs.
	pub(crate) fn collect_disputed(disputed: &BTreeSet<CandidateHash>) -> Vec<CoreIndex> {
		let mut cleaned_up_ids = Vec::new();
		let mut cleaned_up_cores = Vec::new();

		for (para_id, pending_record) in <PendingAvailability<T>>::iter() {
			if disputed.contains(&pending_record.hash) {
				cleaned_up_ids.push(para_id);
				cleaned_up_cores.push(pending_record.core);
			}
		}

		for para_id in cleaned_up_ids {
			let _ = <PendingAvailability<T>>::take(&para_id);
			let _ = <PendingAvailabilityCommitments<T>>::take(&para_id);
		}

		cleaned_up_cores
	}

	/// Forcibly enact the candidate with the given ID as though it had been deemed available
	/// by bitfields.
	///
	/// Is a no-op if there is no candidate pending availability for this para-id.
	/// This should generally not be used but it is useful during execution of Runtime APIs,
	/// where the changes to the state are expected to be discarded directly after.
	pub(crate) fn force_enact(para: ParaId) {
		let pending = <PendingAvailability<T>>::take(&para);
		let commitments = <PendingAvailabilityCommitments<T>>::take(&para);

		if let (Some(pending), Some(commitments)) = (pending, commitments) {
			let candidate =
				CommittedCandidateReceipt { descriptor: pending.descriptor, commitments };

			Self::enact_candidate(
				pending.relay_parent_number,
				candidate,
				pending.backers,
				pending.availability_votes,
				pending.core,
				pending.backing_group,
			);
		}
	}

	/// Returns the `CommittedCandidateReceipt` pending availability for the para provided, if any.
	pub(crate) fn candidate_pending_availability(
		para: ParaId,
	) -> Option<CommittedCandidateReceipt<T::Hash>> {
		<PendingAvailability<T>>::get(&para)
			.map(|p| p.descriptor)
			.and_then(|d| <PendingAvailabilityCommitments<T>>::get(&para).map(move |c| (d, c)))
			.map(|(d, c)| CommittedCandidateReceipt { descriptor: d, commitments: c })
	}

	/// Returns the metadata around the candidate pending availability for the
	/// para provided, if any.
	pub(crate) fn pending_availability(
		para: ParaId,
	) -> Option<CandidatePendingAvailability<T::Hash, T::BlockNumber>> {
		<PendingAvailability<T>>::get(&para)
	}
}

const fn availability_threshold(n_validators: usize) -> usize {
	let mut threshold = (n_validators * 2) / 3;
	threshold += (n_validators * 2) % 3;
	threshold
}

#[derive(derive_more::From, Debug)]
enum AcceptanceCheckErr<BlockNumber> {
	HeadDataTooLarge,
	PrematureCodeUpgrade,
	NewCodeTooLarge,
	ProcessedDownwardMessages(dmp::ProcessedDownwardMessagesAcceptanceErr),
	UpwardMessages(ump::AcceptanceCheckErr),
	HrmpWatermark(hrmp::HrmpWatermarkAcceptanceErr<BlockNumber>),
	OutboundHrmp(hrmp::OutboundHrmpAcceptanceErr),
}

impl<BlockNumber> AcceptanceCheckErr<BlockNumber> {
	/// Returns the same error so that it can be threaded through a needle of `DispatchError` and
	/// ultimately returned from a `Dispatchable`.
	fn strip_into_dispatch_err<T: Config>(self) -> Error<T> {
		use AcceptanceCheckErr::*;
		match self {
			HeadDataTooLarge => Error::<T>::HeadDataTooLarge,
			PrematureCodeUpgrade => Error::<T>::PrematureCodeUpgrade,
			NewCodeTooLarge => Error::<T>::NewCodeTooLarge,
			ProcessedDownwardMessages(_) => Error::<T>::IncorrectDownwardMessageHandling,
			UpwardMessages(_) => Error::<T>::InvalidUpwardMessages,
			HrmpWatermark(_) => Error::<T>::HrmpWatermarkMishandling,
			OutboundHrmp(_) => Error::<T>::InvalidOutboundHrmp,
		}
	}
}

/// A collection of data required for checking a candidate.
pub(crate) struct CandidateCheckContext<T: Config> {
	config: configuration::HostConfiguration<T::BlockNumber>,
	now: T::BlockNumber,
	relay_parent_number: T::BlockNumber,
}

impl<T: Config> CandidateCheckContext<T> {
	pub(crate) fn new(now: T::BlockNumber, relay_parent_number: T::BlockNumber) -> Self {
		Self { config: <configuration::Pallet<T>>::config(), now, relay_parent_number }
	}

	/// Execute verification of the candidate.
	///
	/// Assures:
	///  * correct expected relay parent reference
	///  * collator signature check passes
	///  * code hash of commitments matches current code hash
	///  * para head in the descriptor and commitments match
	pub(crate) fn verify_backed_candidate(
		&self,
		parent_hash: <T as frame_system::Config>::Hash,
		candidate_idx: usize,
		backed_candidate: &BackedCandidate<<T as frame_system::Config>::Hash>,
	) -> Result<(), Error<T>> {
		let para_id = backed_candidate.descriptor().para_id;
		let now = self.now;

		// we require that the candidate is in the context of the parent block.
		ensure!(
			backed_candidate.descriptor().relay_parent == parent_hash,
			Error::<T>::CandidateNotInParentContext,
		);
		ensure!(
			backed_candidate.descriptor().check_collator_signature().is_ok(),
			Error::<T>::NotCollatorSigned,
		);

		let validation_code_hash = <paras::Pallet<T>>::validation_code_hash_at(para_id, now, None)
			// A candidate for a parachain without current validation code is not scheduled.
			.ok_or_else(|| Error::<T>::UnscheduledCandidate)?;
		ensure!(
			backed_candidate.descriptor().validation_code_hash == validation_code_hash,
			Error::<T>::InvalidValidationCodeHash,
		);

		ensure!(
			backed_candidate.descriptor().para_head ==
				backed_candidate.candidate.commitments.head_data.hash(),
			Error::<T>::ParaHeadMismatch,
		);

		if let Err(err) = self.check_validation_outputs(
			para_id,
			&backed_candidate.candidate.commitments.head_data,
			&backed_candidate.candidate.commitments.new_validation_code,
			backed_candidate.candidate.commitments.processed_downward_messages,
			&backed_candidate.candidate.commitments.upward_messages,
			T::BlockNumber::from(backed_candidate.candidate.commitments.hrmp_watermark),
			&backed_candidate.candidate.commitments.horizontal_messages,
		) {
			log::debug!(
				target: LOG_TARGET,
				"Validation outputs checking during inclusion of a candidate {} for parachain `{}` failed: {:?}",
				candidate_idx,
				u32::from(para_id),
				err,
			);
			Err(err.strip_into_dispatch_err::<T>())?;
		};
		Ok(())
	}

	/// Check the given outputs after candidate validation on whether it passes the acceptance
	/// criteria.
	fn check_validation_outputs(
		&self,
		para_id: ParaId,
		head_data: &HeadData,
		new_validation_code: &Option<primitives::v1::ValidationCode>,
		processed_downward_messages: u32,
		upward_messages: &[primitives::v1::UpwardMessage],
		hrmp_watermark: T::BlockNumber,
		horizontal_messages: &[primitives::v1::OutboundHrmpMessage<ParaId>],
	) -> Result<(), AcceptanceCheckErr<T::BlockNumber>> {
		ensure!(
			head_data.0.len() <= self.config.max_head_data_size as _,
			AcceptanceCheckErr::HeadDataTooLarge,
		);

		// if any, the code upgrade attempt is allowed.
		if let Some(new_validation_code) = new_validation_code {
			let valid_upgrade_attempt = <paras::Pallet<T>>::last_code_upgrade(para_id, true)
				.map_or(true, |last| {
					last <= self.relay_parent_number &&
						self.relay_parent_number.saturating_sub(last) >=
							self.config.validation_upgrade_frequency
				});
			ensure!(valid_upgrade_attempt, AcceptanceCheckErr::PrematureCodeUpgrade);
			ensure!(
				new_validation_code.0.len() <= self.config.max_code_size as _,
				AcceptanceCheckErr::NewCodeTooLarge,
			);
		}

		// check if the candidate passes the messaging acceptance criteria
		<dmp::Pallet<T>>::check_processed_downward_messages(para_id, processed_downward_messages)?;
		<ump::Pallet<T>>::check_upward_messages(&self.config, para_id, upward_messages)?;
		<hrmp::Pallet<T>>::check_hrmp_watermark(para_id, self.relay_parent_number, hrmp_watermark)?;
		<hrmp::Pallet<T>>::check_outbound_hrmp(&self.config, para_id, horizontal_messages)?;

		Ok(())
	}
}
