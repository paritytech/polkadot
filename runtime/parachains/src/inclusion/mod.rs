// Copyright (C) Parity Technologies (UK) Ltd.
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

//! The inclusion pallet is responsible for inclusion and availability of scheduled parachains.
//!
//! It is responsible for carrying candidates from being backable to being backed, and then from
//! backed to included.

use crate::{
	configuration::{self, HostConfiguration},
	disputes, dmp, hrmp, paras,
	scheduler::{self, common::CoreAssignment},
	shared::{self, AllowedRelayParentsTracker},
};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_support::{
	defensive,
	pallet_prelude::*,
	traits::{Defensive, EnqueueMessage},
	BoundedSlice,
};
use frame_system::pallet_prelude::*;
use pallet_message_queue::OnQueueChanged;
use parity_scale_codec::{Decode, Encode};
use primitives::{
	supermajority_threshold, well_known_keys, AvailabilityBitfield, BackedCandidate,
	CandidateCommitments, CandidateDescriptor, CandidateHash, CandidateReceipt,
	CommittedCandidateReceipt, CoreIndex, GroupIndex, Hash, HeadData, Id as ParaId,
	SignedAvailabilityBitfields, SigningContext, UpwardMessage, ValidatorId, ValidatorIndex,
	ValidityAttestation,
};
use scale_info::TypeInfo;
use sp_runtime::{traits::One, DispatchError, SaturatedConversion, Saturating};
#[cfg(feature = "std")]
use sp_std::fmt;
use sp_std::{collections::btree_set::BTreeSet, prelude::*};

pub use pallet::*;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

pub trait WeightInfo {
	fn receive_upward_messages(i: u32) -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn receive_upward_messages(_: u32) -> Weight {
		Weight::MAX
	}
}

impl WeightInfo for () {
	fn receive_upward_messages(_: u32) -> Weight {
		Weight::zero()
	}
}

/// Maximum value that `config.max_upward_message_size` can be set to.
///
/// This is used for benchmarking sanely bounding relevant storage items. It is expected from the
/// `configuration` pallet to check these values before setting.
pub const MAX_UPWARD_MESSAGE_SIZE_BOUND: u32 = 128 * 1024;

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

/// A backed candidate pending availability.
#[derive(Encode, Decode, PartialEq, TypeInfo)]
#[cfg_attr(test, derive(Debug))]
pub struct CandidatePendingAvailability<H, N> {
	/// The availability core this is assigned to.
	core: CoreIndex,
	/// The candidate hash.
	hash: CandidateHash,
	/// The candidate descriptor.
	descriptor: CandidateDescriptor<H>,
	/// The received availability votes. One bit per validator.
	availability_votes: BitVec<u8, BitOrderLsb0>,
	/// The backers of the candidate pending availability.
	backers: BitVec<u8, BitOrderLsb0>,
	/// The block number of the relay-parent of the receipt.
	relay_parent_number: N,
	/// The block number of the relay-chain block this was backed in.
	backed_in_number: N,
	/// The group index backing this block.
	backing_group: GroupIndex,
}

impl<H, N> CandidatePendingAvailability<H, N> {
	/// Get the availability votes on the candidate.
	pub(crate) fn availability_votes(&self) -> &BitVec<u8, BitOrderLsb0> {
		&self.availability_votes
	}

	/// Get the relay-chain block number this was backed in.
	pub(crate) fn backed_in_number(&self) -> &N {
		&self.backed_in_number
	}

	/// Get the core index.
	pub(crate) fn core_occupied(&self) -> CoreIndex {
		self.core
	}

	/// Get the candidate hash.
	pub(crate) fn candidate_hash(&self) -> CandidateHash {
		self.hash
	}

	/// Get the candidate descriptor.
	pub(crate) fn candidate_descriptor(&self) -> &CandidateDescriptor<H> {
		&self.descriptor
	}

	/// Get the candidate's relay parent's number.
	pub(crate) fn relay_parent_number(&self) -> N
	where
		N: Clone,
	{
		self.relay_parent_number.clone()
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	pub(crate) fn new(
		core: CoreIndex,
		hash: CandidateHash,
		descriptor: CandidateDescriptor<H>,
		availability_votes: BitVec<u8, BitOrderLsb0>,
		backers: BitVec<u8, BitOrderLsb0>,
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
	pub(crate) core_indices: Vec<(CoreIndex, ParaId)>,
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

/// Number of backing votes we need for a valid backing.
///
/// WARNING: This check has to be kept in sync with the node side checks.
pub fn minimum_backing_votes(n_validators: usize) -> usize {
	// For considerations on this value see:
	// https://github.com/paritytech/polkadot/pull/1656#issuecomment-999734650
	// and
	// https://github.com/paritytech/polkadot/issues/4386
	sp_std::cmp::min(n_validators, 2)
}

/// Reads the footprint of queues for a specific origin type.
pub trait QueueFootprinter {
	type Origin;

	fn message_count(origin: Self::Origin) -> u64;
}

impl QueueFootprinter for () {
	type Origin = UmpQueueId;

	fn message_count(_: Self::Origin) -> u64 {
		0
	}
}

/// Aggregate message origin for the `MessageQueue` pallet.
///
/// Can be extended to serve further use-cases besides just UMP. Is stored in storage, so any change
/// to existing values will require a migration.
#[derive(Encode, Decode, Clone, MaxEncodedLen, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum AggregateMessageOrigin {
	/// Inbound upward message.
	#[codec(index = 0)]
	Ump(UmpQueueId),
}

/// Identifies a UMP queue inside the `MessageQueue` pallet.
///
/// It is written in verbose form since future variants like `Loopback` and `Bridged` are already
/// forseeable.
#[derive(Encode, Decode, Clone, MaxEncodedLen, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum UmpQueueId {
	/// The message originated from this parachain.
	#[codec(index = 0)]
	Para(ParaId),
}

#[cfg(feature = "runtime-benchmarks")]
impl From<u32> for AggregateMessageOrigin {
	fn from(n: u32) -> Self {
		// Some dummy for the benchmarks.
		Self::Ump(UmpQueueId::Para(n.into()))
	}
}

/// The maximal length of a UMP message.
pub type MaxUmpMessageLenOf<T> =
	<<T as Config>::MessageQueue as EnqueueMessage<AggregateMessageOrigin>>::MaxMessageLen;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config
		+ shared::Config
		+ paras::Config
		+ dmp::Config
		+ hrmp::Config
		+ configuration::Config
		+ scheduler::Config
	{
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		type DisputesHandler: disputes::DisputesHandler<BlockNumberFor<Self>>;
		type RewardValidators: RewardValidators;

		/// The system message queue.
		///
		/// The message queue provides general queueing and processing functionality. Currently it
		/// replaces the old `UMP` dispatch queue. Other use-cases can be implemented as well by
		/// adding new variants to `AggregateMessageOrigin`.
		type MessageQueue: EnqueueMessage<AggregateMessageOrigin>;

		/// Weight info for the calls of this pallet.
		type WeightInfo: WeightInfo;
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
		/// Some upward messages have been received and will be processed.
		UpwardMessagesReceived { from: ParaId, count: u32 },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Validator indices are out of order or contains duplicates.
		UnsortedOrDuplicateValidatorIndices,
		/// Dispute statement sets are out of order or contain duplicates.
		UnsortedOrDuplicateDisputeStatementSet,
		/// Backed candidates are out of order (core index) or contain duplicates.
		UnsortedOrDuplicateBackedCandidates,
		/// A different relay parent was provided compared to the on-chain stored one.
		UnexpectedRelayParent,
		/// Availability bitfield has unexpected size.
		WrongBitfieldSize,
		/// Bitfield consists of zeros only.
		BitfieldAllZeros,
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
		/// Scheduled cores out of order.
		ScheduledOutOfOrder,
		/// Head data exceeds the configured maximum.
		HeadDataTooLarge,
		/// Code upgrade prematurely.
		PrematureCodeUpgrade,
		/// Output code is too large
		NewCodeTooLarge,
		/// The candidate's relay-parent was not allowed. Either it was
		/// not recent enough or it didn't advance based on the last parachain block.
		DisallowedRelayParent,
		/// Failed to compute group index for the core: either it's out of bounds
		/// or the relay parent doesn't belong to the current session.
		InvalidAssignment,
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
		/// The `para_head` hash in the candidate descriptor doesn't match the hash of the actual
		/// para head in the commitments.
		ParaHeadMismatch,
		/// A bitfield that references a freed core,
		/// either intentionally or as part of a concluded
		/// invalid dispute.
		BitfieldReferencesFreedCore,
	}

	/// The latest bitfield for each validator, referred to by their index in the validator set.
	#[pallet::storage]
	pub(crate) type AvailabilityBitfields<T: Config> =
		StorageMap<_, Twox64Concat, ValidatorIndex, AvailabilityBitfieldRecord<BlockNumberFor<T>>>;

	/// Candidates pending availability by `ParaId`.
	#[pallet::storage]
	pub(crate) type PendingAvailability<T: Config> = StorageMap<
		_,
		Twox64Concat,
		ParaId,
		CandidatePendingAvailability<T::Hash, BlockNumberFor<T>>,
	>;

	/// The commitments of candidates pending availability, by `ParaId`.
	#[pallet::storage]
	pub(crate) type PendingAvailabilityCommitments<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, CandidateCommitments>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

const LOG_TARGET: &str = "runtime::inclusion";

/// The reason that a candidate's outputs were rejected for.
#[derive(derive_more::From)]
#[cfg_attr(feature = "std", derive(Debug))]
enum AcceptanceCheckErr<BlockNumber> {
	HeadDataTooLarge,
	/// Code upgrades are not permitted at the current time.
	PrematureCodeUpgrade,
	/// The new runtime blob is too large.
	NewCodeTooLarge,
	/// The candidate violated this DMP acceptance criteria.
	ProcessedDownwardMessages(dmp::ProcessedDownwardMessagesAcceptanceErr),
	/// The candidate violated this UMP acceptance criteria.
	UpwardMessages(UmpAcceptanceCheckErr),
	/// The candidate violated this HRMP watermark acceptance criteria.
	HrmpWatermark(hrmp::HrmpWatermarkAcceptanceErr<BlockNumber>),
	/// The candidate violated this outbound HRMP acceptance criteria.
	OutboundHrmp(hrmp::OutboundHrmpAcceptanceErr),
}

/// An error returned by [`check_upward_messages`] that indicates a violation of one of acceptance
/// criteria rules.
#[cfg_attr(test, derive(PartialEq))]
pub enum UmpAcceptanceCheckErr {
	/// The maximal number of messages that can be submitted in one batch was exceeded.
	MoreMessagesThanPermitted { sent: u32, permitted: u32 },
	/// The maximal size of a single message was exceeded.
	MessageSize { idx: u32, msg_size: u32, max_size: u32 },
	/// The allowed number of messages in the queue was exceeded.
	CapacityExceeded { count: u64, limit: u64 },
	/// The allowed combined message size in the queue was exceeded.
	TotalSizeExceeded { total_size: u64, limit: u64 },
	/// A para-chain cannot send UMP messages while it is offboarding.
	IsOffboarding,
}

#[cfg(feature = "std")]
impl fmt::Debug for UmpAcceptanceCheckErr {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		match *self {
			UmpAcceptanceCheckErr::MoreMessagesThanPermitted { sent, permitted } => write!(
				fmt,
				"more upward messages than permitted by config ({} > {})",
				sent, permitted,
			),
			UmpAcceptanceCheckErr::MessageSize { idx, msg_size, max_size } => write!(
				fmt,
				"upward message idx {} larger than permitted by config ({} > {})",
				idx, msg_size, max_size,
			),
			UmpAcceptanceCheckErr::CapacityExceeded { count, limit } => write!(
				fmt,
				"the ump queue would have more items than permitted by config ({} > {})",
				count, limit,
			),
			UmpAcceptanceCheckErr::TotalSizeExceeded { total_size, limit } => write!(
				fmt,
				"the ump queue would have grown past the max size permitted by config ({} > {})",
				total_size, limit,
			),
			UmpAcceptanceCheckErr::IsOffboarding =>
				write!(fmt, "upward message rejected because the para is off-boarding",),
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: BlockNumberFor<T>) -> Weight {
		Weight::zero()
	}

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() {}

	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		_notification: &crate::initializer::SessionChangeNotification<BlockNumberFor<T>>,
		outgoing_paras: &[ParaId],
	) {
		// unlike most drain methods, drained elements are not cleared on `Drop` of the iterator
		// and require consumption.
		for _ in <PendingAvailabilityCommitments<T>>::drain() {}
		for _ in <PendingAvailability<T>>::drain() {}
		for _ in <AvailabilityBitfields<T>>::drain() {}

		Self::cleanup_outgoing_ump_dispatch_queues(outgoing_paras);
	}

	pub(crate) fn cleanup_outgoing_ump_dispatch_queues(outgoing: &[ParaId]) {
		for outgoing_para in outgoing {
			Self::cleanup_outgoing_ump_dispatch_queue(*outgoing_para);
		}
	}

	pub(crate) fn cleanup_outgoing_ump_dispatch_queue(para: ParaId) {
		T::MessageQueue::sweep_queue(AggregateMessageOrigin::Ump(UmpQueueId::Para(para)));
	}

	/// Extract the freed cores based on cores that became available.
	///
	/// Bitfields are expected to have been sanitized already. E.g. via `sanitize_bitfields`!
	///
	/// Updates storage items `PendingAvailability` and `AvailabilityBitfields`.
	///
	/// Returns a `Vec` of `CandidateHash`es and their respective `AvailabilityCore`s that became
	/// available, and cores free.
	pub(crate) fn update_pending_availability_and_get_freed_cores<F>(
		expected_bits: usize,
		validators: &[ValidatorId],
		signed_bitfields: SignedAvailabilityBitfields,
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
				let validator_idx = signed_bitfield.validator_index();
				let checked_bitfield = signed_bitfield.into_payload();
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

				// defensive check - this is constructed by loading the availability bitfield
				// record, which is always `Some` if the core is occupied - that's why we're here.
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
			.flatten()
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

				freed_cores.push((pending_availability.core, pending_availability.hash));
			} else {
				<PendingAvailability<T>>::insert(&para_id, &pending_availability);
			}
		}

		freed_cores
	}

	/// Process candidates that have been backed. Provide the relay storage root, a set of
	/// candidates and scheduled cores.
	///
	/// Both should be sorted ascending by core index, and the candidates should be a subset of
	/// scheduled cores. If these conditions are not met, the execution of the function fails.
	pub(crate) fn process_candidates<GV>(
		allowed_relay_parents: &AllowedRelayParentsTracker<T::Hash, BlockNumberFor<T>>,
		candidates: Vec<BackedCandidate<T::Hash>>,
		scheduled: Vec<CoreAssignment<BlockNumberFor<T>>>,
		group_validators: GV,
	) -> Result<ProcessedCandidates<T::Hash>, DispatchError>
	where
		GV: Fn(GroupIndex) -> Option<Vec<ValidatorIndex>>,
	{
		let now = <frame_system::Pallet<T>>::block_number();

		ensure!(candidates.len() <= scheduled.len(), Error::<T>::UnscheduledCandidate);

		if scheduled.is_empty() {
			return Ok(ProcessedCandidates::default())
		}

		let validators = shared::Pallet::<T>::active_validator_keys();

		// Collect candidate receipts with backers.
		let mut candidate_receipt_with_backing_validator_indices =
			Vec::with_capacity(candidates.len());

		// Do all checks before writing storage.
		let core_indices_and_backers = {
			let mut skip = 0;
			let mut core_indices_and_backers = Vec::with_capacity(candidates.len());
			let mut last_core = None;

			let mut check_assignment_in_order =
				|assignment: &CoreAssignment<BlockNumberFor<T>>| -> DispatchResult {
					ensure!(
						last_core.map_or(true, |core| assignment.core > core),
						Error::<T>::ScheduledOutOfOrder,
					);

					last_core = Some(assignment.core);
					Ok(())
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
			'next_backed_candidate: for (candidate_idx, backed_candidate) in
				candidates.iter().enumerate()
			{
				let relay_parent_hash = backed_candidate.descriptor().relay_parent;
				let para_id = backed_candidate.descriptor().para_id;

				let prev_context = <paras::Pallet<T>>::para_most_recent_context(para_id);

				let check_ctx = CandidateCheckContext::<T>::new(prev_context);
				let signing_context = SigningContext {
					parent_hash: relay_parent_hash,
					session_index: shared::Pallet::<T>::session_index(),
				};

				let relay_parent_number = match check_ctx.verify_backed_candidate(
					&allowed_relay_parents,
					candidate_idx,
					backed_candidate,
				)? {
					Err(FailedToCreatePVD) => {
						log::debug!(
							target: LOG_TARGET,
							"Failed to create PVD for candidate {}",
							candidate_idx,
						);
						// We don't want to error out here because it will
						// brick the relay-chain. So we return early without
						// doing anything.
						return Ok(ProcessedCandidates::default())
					},
					Ok(rpn) => rpn,
				};

				let para_id = backed_candidate.descriptor().para_id;
				let mut backers = bitvec::bitvec![u8, BitOrderLsb0; 0; validators.len()];

				for (i, core_assignment) in scheduled[skip..].iter().enumerate() {
					check_assignment_in_order(core_assignment)?;

					if para_id == core_assignment.paras_entry.para_id() {
						ensure!(
							<PendingAvailability<T>>::get(&para_id).is_none() &&
								<PendingAvailabilityCommitments<T>>::get(&para_id).is_none(),
							Error::<T>::CandidateScheduledBeforeParaFree,
						);

						// account for already skipped, and then skip this one.
						skip = i + skip + 1;

						// The candidate based upon relay parent `N` should be backed by a group
						// assigned to core at block `N + 1`. Thus, `relay_parent_number + 1`
						// will always land in the current session.
						let group_idx = <scheduler::Pallet<T>>::group_assigned_to_core(
							core_assignment.core,
							relay_parent_number + One::one(),
						)
						.ok_or_else(|| {
							log::warn!(
								target: LOG_TARGET,
								"Failed to compute group index for candidate {}",
								candidate_idx
							);
							Error::<T>::InvalidAssignment
						})?;
						let group_vals = group_validators(group_idx)
							.ok_or_else(|| Error::<T>::InvalidGroupIndex)?;

						// check the signatures in the backing and that it is a majority.
						{
							let maybe_amount_validated = primitives::check_candidate_backing(
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
									amount_validated >= minimum_backing_votes(group_vals.len()),
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
							(core_assignment.core, core_assignment.paras_entry.para_id()),
							backers,
							group_idx,
							relay_parent_number,
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
		let core_indices = core_indices_and_backers.iter().map(|(c, ..)| *c).collect();
		for (candidate, (core, backers, group, relay_parent_number)) in
			candidates.into_iter().zip(core_indices_and_backers)
		{
			let para_id = candidate.descriptor().para_id;

			// initialize all availability votes to 0.
			let availability_votes: BitVec<u8, BitOrderLsb0> =
				bitvec::bitvec![u8, BitOrderLsb0; 0; validators.len()];

			Self::deposit_event(Event::<T>::CandidateBacked(
				candidate.candidate.to_plain(),
				candidate.candidate.commitments.head_data.clone(),
				core.0,
				group,
			));

			let candidate_hash = candidate.candidate.hash();

			let (descriptor, commitments) =
				(candidate.candidate.descriptor, candidate.candidate.commitments);

			<PendingAvailability<T>>::insert(
				&para_id,
				CandidatePendingAvailability {
					core: core.0,
					hash: candidate_hash,
					descriptor,
					availability_votes,
					relay_parent_number,
					backers: backers.to_bitvec(),
					backed_in_number: now,
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
		relay_parent_number: BlockNumberFor<T>,
		validation_outputs: primitives::CandidateCommitments,
	) -> bool {
		let prev_context = <paras::Pallet<T>>::para_most_recent_context(para_id);
		let check_ctx = CandidateCheckContext::<T>::new(prev_context);

		if check_ctx
			.check_validation_outputs(
				para_id,
				relay_parent_number,
				&validation_outputs.head_data,
				&validation_outputs.new_validation_code,
				validation_outputs.processed_downward_messages,
				&validation_outputs.upward_messages,
				BlockNumberFor::<T>::from(validation_outputs.hrmp_watermark),
				&validation_outputs.horizontal_messages,
			)
			.is_err()
		{
			log::debug!(
				target: LOG_TARGET,
				"Validation outputs checking for parachain `{}` failed",
				u32::from(para_id),
			);
			false
		} else {
			true
		}
	}

	fn enact_candidate(
		relay_parent_number: BlockNumberFor<T>,
		receipt: CommittedCandidateReceipt<T::Hash>,
		backers: BitVec<u8, BitOrderLsb0>,
		availability_votes: BitVec<u8, BitOrderLsb0>,
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
			// Block number of candidate's inclusion.
			let now = <frame_system::Pallet<T>>::block_number();

			weight.saturating_add(<paras::Pallet<T>>::schedule_code_upgrade(
				receipt.descriptor.para_id,
				new_code,
				now,
				&config,
			));
		}

		// enact the messaging facet of the candidate.
		weight.saturating_accrue(<dmp::Pallet<T>>::prune_dmq(
			receipt.descriptor.para_id,
			commitments.processed_downward_messages,
		));
		weight.saturating_accrue(Self::receive_upward_messages(
			receipt.descriptor.para_id,
			commitments.upward_messages.as_slice(),
		));
		weight.saturating_accrue(<hrmp::Pallet<T>>::prune_hrmp(
			receipt.descriptor.para_id,
			BlockNumberFor::<T>::from(commitments.hrmp_watermark),
		));
		weight.saturating_accrue(<hrmp::Pallet<T>>::queue_outbound_hrmp(
			receipt.descriptor.para_id,
			commitments.horizontal_messages,
		));

		Self::deposit_event(Event::<T>::CandidateIncluded(
			plain,
			commitments.head_data.clone(),
			core_index,
			backing_group,
		));

		weight.saturating_add(<paras::Pallet<T>>::note_new_head(
			receipt.descriptor.para_id,
			commitments.head_data,
			relay_parent_number,
		))
	}

	pub(crate) fn relay_dispatch_queue_size(para_id: ParaId) -> (u32, u32) {
		let fp = T::MessageQueue::footprint(AggregateMessageOrigin::Ump(UmpQueueId::Para(para_id)));
		(fp.count as u32, fp.size as u32)
	}

	/// Check that all the upward messages sent by a candidate pass the acceptance criteria.
	pub(crate) fn check_upward_messages(
		config: &HostConfiguration<BlockNumberFor<T>>,
		para: ParaId,
		upward_messages: &[UpwardMessage],
	) -> Result<(), UmpAcceptanceCheckErr> {
		// Cannot send UMP messages while off-boarding.
		if <paras::Pallet<T>>::is_offboarding(para) {
			ensure!(upward_messages.is_empty(), UmpAcceptanceCheckErr::IsOffboarding);
		}

		let additional_msgs = upward_messages.len() as u32;
		if additional_msgs > config.max_upward_message_num_per_candidate {
			return Err(UmpAcceptanceCheckErr::MoreMessagesThanPermitted {
				sent: additional_msgs,
				permitted: config.max_upward_message_num_per_candidate,
			})
		}

		let (para_queue_count, mut para_queue_size) = Self::relay_dispatch_queue_size(para);

		if para_queue_count.saturating_add(additional_msgs) > config.max_upward_queue_count {
			return Err(UmpAcceptanceCheckErr::CapacityExceeded {
				count: para_queue_count.saturating_add(additional_msgs).into(),
				limit: config.max_upward_queue_count.into(),
			})
		}

		for (idx, msg) in upward_messages.into_iter().enumerate() {
			let msg_size = msg.len() as u32;
			if msg_size > config.max_upward_message_size {
				return Err(UmpAcceptanceCheckErr::MessageSize {
					idx: idx as u32,
					msg_size,
					max_size: config.max_upward_message_size,
				})
			}
			// make sure that the queue is not overfilled.
			// we do it here only once since returning false invalidates the whole relay-chain
			// block.
			if para_queue_size.saturating_add(msg_size) > config.max_upward_queue_size {
				return Err(UmpAcceptanceCheckErr::TotalSizeExceeded {
					total_size: para_queue_size.saturating_add(msg_size).into(),
					limit: config.max_upward_queue_size.into(),
				})
			}
			para_queue_size.saturating_accrue(msg_size);
		}

		Ok(())
	}

	/// Enqueues `upward_messages` from a `para`'s accepted candidate block.
	///
	/// This function is infallible since the candidate was already accepted and we therefore need
	/// to deal with the messages as given. Messages that are too long will be ignored since such
	/// candidates should have already been rejected in [`Self::check_upward_messages`].
	pub(crate) fn receive_upward_messages(para: ParaId, upward_messages: &[Vec<u8>]) -> Weight {
		let bounded = upward_messages
			.iter()
			.filter_map(|d| {
				BoundedSlice::try_from(&d[..])
					.map_err(|e| {
						defensive!("Accepted candidate contains too long msg, len=", d.len());
						e
					})
					.ok()
			})
			.collect();
		Self::receive_bounded_upward_messages(para, bounded)
	}

	/// Enqueues storage-bounded `upward_messages` from a `para`'s accepted candidate block.
	pub(crate) fn receive_bounded_upward_messages(
		para: ParaId,
		messages: Vec<BoundedSlice<'_, u8, MaxUmpMessageLenOf<T>>>,
	) -> Weight {
		let count = messages.len() as u32;
		if count == 0 {
			return Weight::zero()
		}

		T::MessageQueue::enqueue_messages(
			messages.into_iter(),
			AggregateMessageOrigin::Ump(UmpQueueId::Para(para)),
		);
		let weight = <T as Config>::WeightInfo::receive_upward_messages(count);
		Self::deposit_event(Event::UpwardMessagesReceived { from: para, count });
		weight
	}

	/// Cleans up all paras pending availability that the predicate returns true for.
	///
	/// The predicate accepts the index of the core and the block number the core has been occupied
	/// since (i.e. the block number the candidate was backed at in this fork of the relay chain).
	///
	/// Returns a vector of cleaned-up core IDs.
	pub(crate) fn collect_pending(
		pred: impl Fn(CoreIndex, BlockNumberFor<T>) -> bool,
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
	) -> Option<CandidatePendingAvailability<T::Hash, BlockNumberFor<T>>> {
		<PendingAvailability<T>>::get(&para)
	}
}

const fn availability_threshold(n_validators: usize) -> usize {
	supermajority_threshold(n_validators)
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

impl<T: Config> OnQueueChanged<AggregateMessageOrigin> for Pallet<T> {
	// Write back the remaining queue capacity into `relay_dispatch_queue_remaining_capacity`.
	fn on_queue_changed(origin: AggregateMessageOrigin, count: u64, size: u64) {
		let para = match origin {
			AggregateMessageOrigin::Ump(UmpQueueId::Para(p)) => p,
		};
		let (count, size) = (count.saturated_into(), size.saturated_into());
		// TODO paritytech/polkadot#6283: Remove all usages of `relay_dispatch_queue_size`
		#[allow(deprecated)]
		well_known_keys::relay_dispatch_queue_size_typed(para).set((count, size));

		let config = <configuration::Pallet<T>>::config();
		let remaining_count = config.max_upward_queue_count.saturating_sub(count);
		let remaining_size = config.max_upward_queue_size.saturating_sub(size);
		well_known_keys::relay_dispatch_queue_remaining_capacity(para)
			.set((remaining_count, remaining_size));
	}
}

/// A collection of data required for checking a candidate.
pub(crate) struct CandidateCheckContext<T: Config> {
	config: configuration::HostConfiguration<BlockNumberFor<T>>,
	prev_context: Option<BlockNumberFor<T>>,
}

/// An error indicating that creating Persisted Validation Data failed
/// while checking a candidate's validity.
pub(crate) struct FailedToCreatePVD;

impl<T: Config> CandidateCheckContext<T> {
	pub(crate) fn new(prev_context: Option<BlockNumberFor<T>>) -> Self {
		Self { config: <configuration::Pallet<T>>::config(), prev_context }
	}

	/// Execute verification of the candidate.
	///
	/// Assures:
	///  * relay-parent in-bounds
	///  * collator signature check passes
	///  * code hash of commitments matches current code hash
	///  * para head in the descriptor and commitments match
	///
	/// Returns the relay-parent block number.
	pub(crate) fn verify_backed_candidate(
		&self,
		allowed_relay_parents: &AllowedRelayParentsTracker<T::Hash, BlockNumberFor<T>>,
		candidate_idx: usize,
		backed_candidate: &BackedCandidate<<T as frame_system::Config>::Hash>,
	) -> Result<Result<BlockNumberFor<T>, FailedToCreatePVD>, Error<T>> {
		let para_id = backed_candidate.descriptor().para_id;
		let relay_parent = backed_candidate.descriptor().relay_parent;

		// Check that the relay-parent is one of the allowed relay-parents.
		let (relay_parent_storage_root, relay_parent_number) = {
			match allowed_relay_parents.acquire_info(relay_parent, self.prev_context) {
				None => return Err(Error::<T>::DisallowedRelayParent),
				Some(info) => info,
			}
		};

		{
			let persisted_validation_data = match crate::util::make_persisted_validation_data::<T>(
				para_id,
				relay_parent_number,
				relay_parent_storage_root,
			)
			.defensive_proof("the para is registered")
			{
				Some(l) => l,
				None => return Ok(Err(FailedToCreatePVD)),
			};

			let expected = persisted_validation_data.hash();

			ensure!(
				expected == backed_candidate.descriptor().persisted_validation_data_hash,
				Error::<T>::ValidationDataHashMismatch,
			);
		}

		ensure!(
			backed_candidate.descriptor().check_collator_signature().is_ok(),
			Error::<T>::NotCollatorSigned,
		);

		let validation_code_hash = <paras::Pallet<T>>::current_code_hash(para_id)
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
			relay_parent_number,
			&backed_candidate.candidate.commitments.head_data,
			&backed_candidate.candidate.commitments.new_validation_code,
			backed_candidate.candidate.commitments.processed_downward_messages,
			&backed_candidate.candidate.commitments.upward_messages,
			BlockNumberFor::<T>::from(backed_candidate.candidate.commitments.hrmp_watermark),
			&backed_candidate.candidate.commitments.horizontal_messages,
		) {
			log::debug!(
				target: LOG_TARGET,
				"Validation outputs checking during inclusion of a candidate {} for parachain `{}` failed",
				candidate_idx,
				u32::from(para_id),
			);
			Err(err.strip_into_dispatch_err::<T>())?;
		};
		Ok(Ok(relay_parent_number))
	}

	/// Check the given outputs after candidate validation on whether it passes the acceptance
	/// criteria.
	///
	/// The things that are checked can be roughly divided into limits and minimums.
	///
	/// Limits are things like max message queue sizes and max head data size.
	///
	/// Minimums are things like the minimum amount of messages that must be processed
	/// by the parachain block.
	///
	/// Limits are checked against the current state. The parachain block must be acceptable
	/// by the current relay-chain state regardless of whether it was acceptable at some relay-chain
	/// state in the past.
	///
	/// Minimums are checked against the current state but modulated by
	/// considering the information available at the relay-parent of the parachain block.
	fn check_validation_outputs(
		&self,
		para_id: ParaId,
		relay_parent_number: BlockNumberFor<T>,
		head_data: &HeadData,
		new_validation_code: &Option<primitives::ValidationCode>,
		processed_downward_messages: u32,
		upward_messages: &[primitives::UpwardMessage],
		hrmp_watermark: BlockNumberFor<T>,
		horizontal_messages: &[primitives::OutboundHrmpMessage<ParaId>],
	) -> Result<(), AcceptanceCheckErr<BlockNumberFor<T>>> {
		ensure!(
			head_data.0.len() <= self.config.max_head_data_size as _,
			AcceptanceCheckErr::HeadDataTooLarge,
		);

		// if any, the code upgrade attempt is allowed.
		if let Some(new_validation_code) = new_validation_code {
			ensure!(
				<paras::Pallet<T>>::can_upgrade_validation_code(para_id),
				AcceptanceCheckErr::PrematureCodeUpgrade,
			);
			ensure!(
				new_validation_code.0.len() <= self.config.max_code_size as _,
				AcceptanceCheckErr::NewCodeTooLarge,
			);
		}

		// check if the candidate passes the messaging acceptance criteria
		<dmp::Pallet<T>>::check_processed_downward_messages(
			para_id,
			relay_parent_number,
			processed_downward_messages,
		)?;
		Pallet::<T>::check_upward_messages(&self.config, para_id, upward_messages)?;
		<hrmp::Pallet<T>>::check_hrmp_watermark(para_id, relay_parent_number, hrmp_watermark)?;
		<hrmp::Pallet<T>>::check_outbound_hrmp(&self.config, para_id, horizontal_messages)?;

		Ok(())
	}
}

impl<T: Config> QueueFootprinter for Pallet<T> {
	type Origin = UmpQueueId;

	fn message_count(origin: Self::Origin) -> u64 {
		T::MessageQueue::footprint(AggregateMessageOrigin::Ump(origin)).count
	}
}
