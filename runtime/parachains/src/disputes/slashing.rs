// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Dispute slashing types for the substrate offences pallet.

use crate::{
	initializer::SessionChangeNotification,
	session_info::{AccountId, IdentificationTuple},
};
use frame_support::{
	traits::{Get, KeyOwnerProofSystem, ValidatorSet, ValidatorSetWithIdentification},
	weights::{Pays, Weight},
};
use parity_scale_codec::{Decode, Encode};
use primitives::v2::{CandidateHash, SessionIndex, ValidatorId, ValidatorIndex};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::Convert,
	transaction_validity::{
		InvalidTransaction, TransactionPriority, TransactionSource, TransactionValidity,
		TransactionValidityError, ValidTransaction,
	},
	DispatchResult, KeyTypeId, Perbill, RuntimeDebug,
};
use sp_session::{GetSessionNumber, GetValidatorCount};
use sp_staking::offence::{DisableStrategy, Kind, Offence, OffenceError, ReportOffence};
use sp_std::{collections::btree_set::BTreeSet, prelude::*};

const LOG_TARGET: &str = "runtime::slashing";

/// Timeslots should uniquely identify offences
/// and are used for the offence deduplication.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub struct DisputesTimeSlot {
	// TODO: `TimeSlot` docs says it should fit into `u128`
	// any proofs?

	// The order of these matters for `derive(Ord)`.
	session_index: SessionIndex,
	candidate_hash: CandidateHash,
}

impl DisputesTimeSlot {
	pub fn new(session_index: SessionIndex, candidate_hash: CandidateHash) -> Self {
		Self { session_index, candidate_hash }
	}
}

/// An offence that is filed when a series of validators lost a dispute
/// about an invalid candidate.
#[derive(RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct ForInvalidOffence<KeyOwnerIdentification> {
	/// The size of the validator set in that session.
	/// Note: this includes not only parachain validators.
	pub validator_set_count: u32,
	/// Should be unique per dispute.
	pub time_slot: DisputesTimeSlot,
	/// Staking information about the validators that lost the dispute
	/// needed for slashing.
	pub offenders: Vec<KeyOwnerIdentification>,
}

impl<Offender> Offence<Offender> for ForInvalidOffence<Offender>
where
	Offender: Clone,
{
	const ID: Kind = *b"disputes:invalid";

	type TimeSlot = DisputesTimeSlot;

	fn offenders(&self) -> Vec<Offender> {
		self.offenders.clone()
	}

	fn session_index(&self) -> SessionIndex {
		self.time_slot.session_index
	}

	fn validator_set_count(&self) -> u32 {
		self.validator_set_count
	}

	fn time_slot(&self) -> Self::TimeSlot {
		self.time_slot.clone()
	}

	fn disable_strategy(&self) -> DisableStrategy {
		// The default is `DisableStrategy::DisableWhenSlashed`
		// which is true for this offence type, but we're being explicit
		DisableStrategy::Always
	}

	fn slash_fraction(_offenders: u32, _validator_set_count: u32) -> Perbill {
		Perbill::from_percent(100) // TODO
	}
}

// TODO: this can include a multiplier to make slashing worse
// and enable disabling
/// An offence that is filed when a series of validators lost a dispute
/// about an valid candidate.
#[derive(RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct AgainstValidOffence<KeyOwnerIdentification> {
	/// The size of the validator set in that session.
	/// Note: this includes not only parachain validators.
	pub validator_set_count: u32,
	/// Should be unique per dispute.
	pub time_slot: DisputesTimeSlot,
	/// Staking information about the validators that lost the dispute
	/// needed for slashing.
	pub offenders: Vec<KeyOwnerIdentification>,
}

impl<Offender> Offence<Offender> for AgainstValidOffence<Offender>
where
	Offender: Clone,
{
	const ID: Kind = *b"disputes:valid::";

	type TimeSlot = DisputesTimeSlot;

	fn offenders(&self) -> Vec<Offender> {
		self.offenders.clone()
	}

	fn session_index(&self) -> SessionIndex {
		self.time_slot.session_index
	}

	fn validator_set_count(&self) -> u32 {
		self.validator_set_count
	}

	fn time_slot(&self) -> Self::TimeSlot {
		self.time_slot.clone()
	}

	fn disable_strategy(&self) -> DisableStrategy {
		DisableStrategy::Never
	}

	fn slash_fraction(_offenders: u32, _validator_set_count: u32) -> Perbill {
		Perbill::from_percent(1) // TODO
	}
}

/// This type implements `PunishValidators`.
pub struct SlashValidatorsForDisputes<C, R> {
	_phantom: sp_std::marker::PhantomData<(C, R)>,
}

impl<C, R> Default for SlashValidatorsForDisputes<C, R> {
	fn default() -> Self {
		Self { _phantom: Default::default() }
	}
}

impl<T, R> SlashValidatorsForDisputes<T, R>
where
	T: Config + crate::session_info::Config,
	R: ReportOffence<
			AccountId<T>,
			IdentificationTuple<T>,
			ForInvalidOffence<IdentificationTuple<T>>,
		> + ReportOffence<
			AccountId<T>,
			IdentificationTuple<T>,
			AgainstValidOffence<IdentificationTuple<T>>,
		>,
{
	/// If in the current session, returns the identified validators
	/// along with the validator set count for that session. `None` otherwise.
	fn maybe_identify_validators(
		session_index: SessionIndex,
		account_ids: &[AccountId<T>],
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) -> Option<(Vec<IdentificationTuple<T>>, u32)> {
		// We use `ValidatorSet::session_index` and not `shared::Pallet<T>::session_index()`
		// because at the first block of a new era, the `IdentificationOf` of a validator
		// in the previous session might be missing, while `shared` pallet would return
		// the same session index as being updated at the end of the block.
		let current_session = T::ValidatorSet::session_index();
		if session_index == current_session {
			let validator_set_count = account_ids.len() as u32;
			let fully_identified = validators
				.into_iter()
				.flat_map(|i| account_ids.get(i.0 as usize).cloned())
				.filter_map(|id| {
					<T::ValidatorSet as ValidatorSetWithIdentification<T::AccountId>>::IdentificationOf::convert(
						id.clone()
					).map(|full_id| (id, full_id))
				})
				.collect::<Vec<IdentificationTuple<T>>>();
			return Some((fully_identified, validator_set_count))
		}
		None
	}
}

impl<T, R> super::PunishValidators for SlashValidatorsForDisputes<T, R>
where
	T: Config + crate::session_info::Config,
	// TODO: use HandleReports instead?
	R: ReportOffence<
			AccountId<T>,
			IdentificationTuple<T>,
			ForInvalidOffence<IdentificationTuple<T>>,
		> + ReportOffence<
			AccountId<T>,
			IdentificationTuple<T>,
			AgainstValidOffence<IdentificationTuple<T>>,
		>,
{
	fn punish_for_invalid(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		losers: impl IntoIterator<Item = ValidatorIndex>,
		winners: impl IntoIterator<Item = ValidatorIndex>,
	) {
		let losers: Losers = losers.into_iter().collect();
		if losers.is_empty() {
			// Nothing to do
			return
		}
		let account_keys = crate::session_info::Pallet::<T>::account_keys(session_index);
		let account_ids = match account_keys {
			Some(account_keys) => account_keys,
			None => {
				// TODO: warn
				return
			},
		};
		let winners: Winners<T> = winners
			.into_iter()
			.filter_map(|i| account_ids.get(i.0 as usize).cloned())
			.collect();
		let maybe =
			Self::maybe_identify_validators(session_index, &account_ids, losers.iter().cloned());
		if let Some((offenders, validator_set_count)) = maybe {
			let time_slot = DisputesTimeSlot::new(session_index, candidate_hash);
			let offence = ForInvalidOffence { validator_set_count, time_slot, offenders };
			// TODO: log error?
			let _ = R::report_offence(winners, offence);
			return
		}

		<PendingForInvalidLosers<T>>::insert(session_index, candidate_hash, losers);
		<ForInvalidWinners<T>>::insert(session_index, candidate_hash, winners);
	}

	fn punish_against_valid(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		losers: impl IntoIterator<Item = ValidatorIndex>,
		winners: impl IntoIterator<Item = ValidatorIndex>,
	) {
		let losers: Losers = losers.into_iter().collect();
		if losers.is_empty() {
			// Nothing to do
			return
		}
		let account_keys = crate::session_info::Pallet::<T>::account_keys(session_index);
		let account_ids = match account_keys {
			Some(account_keys) => account_keys,
			None => {
				// TODO: warn
				return
			},
		};
		let winners: Winners<T> = winners
			.into_iter()
			.filter_map(|i| account_ids.get(i.0 as usize).cloned())
			.collect();
		let maybe =
			Self::maybe_identify_validators(session_index, &account_ids, losers.iter().cloned());
		if let Some((offenders, validator_set_count)) = maybe {
			let time_slot = DisputesTimeSlot::new(session_index, candidate_hash);
			let offence = AgainstValidOffence { validator_set_count, time_slot, offenders };
			// TODO: log error?
			let _ = R::report_offence(winners, offence);
			return
		}

		<PendingAgainstValidLosers<T>>::insert(session_index, candidate_hash, losers);
		<AgainstValidWinners<T>>::insert(session_index, candidate_hash, winners);
	}
}

#[derive(PartialEq, Eq, Clone, Copy, Encode, Decode, RuntimeDebug, TypeInfo)]
pub enum SlashingOffenceKind {
	#[codec(index = 0)]
	ForInvalid,
	#[codec(index = 1)]
	AgainstValid,
}

/// We store most of the information about a lost dispute on chain.
/// This struct is required to identify and verify it.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub struct DisputeProof {
	/// Time slot when the dispute occured.
	pub time_slot: DisputesTimeSlot,
	/// The dispute outcome.
	pub kind: SlashingOffenceKind,
	/// The index of the validator who lost a dispute.
	pub validator_index: ValidatorIndex,
	/// The parachain session key of the validator.
	pub validator_id: ValidatorId,
}

// TODO: docs
pub type Losers = BTreeSet<ValidatorIndex>;
pub type Winners<T> = Vec<AccountId<T>>;

pub trait SlashingOffence<KeyOwnerIdentification>: Offence<KeyOwnerIdentification> {
	/// Create a new dispute offence using the given details.
	fn new(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		validator_set_count: u32,
		offender: KeyOwnerIdentification,
	) -> Self;

	/// Create a new dispute offence time slot.
	fn new_time_slot(session_index: SessionIndex, candidate_hash: CandidateHash) -> Self::TimeSlot;
}

impl<KeyOwnerIdentification> SlashingOffence<KeyOwnerIdentification>
	for ForInvalidOffence<KeyOwnerIdentification>
where
	KeyOwnerIdentification: Clone,
{
	fn new(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		validator_set_count: u32,
		offender: KeyOwnerIdentification,
	) -> Self {
		let time_slot = Self::new_time_slot(session_index, candidate_hash);
		Self { time_slot, validator_set_count, offenders: vec![offender] }
	}

	fn new_time_slot(session_index: SessionIndex, candidate_hash: CandidateHash) -> Self::TimeSlot {
		DisputesTimeSlot { session_index, candidate_hash }
	}
}

impl<KeyOwnerIdentification> SlashingOffence<KeyOwnerIdentification>
	for AgainstValidOffence<KeyOwnerIdentification>
where
	KeyOwnerIdentification: Clone,
{
	fn new(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		validator_set_count: u32,
		offender: KeyOwnerIdentification,
	) -> Self {
		let time_slot = Self::new_time_slot(session_index, candidate_hash);
		Self { time_slot, validator_set_count, offenders: vec![offender] }
	}

	fn new_time_slot(session_index: SessionIndex, candidate_hash: CandidateHash) -> Self::TimeSlot {
		DisputesTimeSlot { session_index, candidate_hash }
	}
}

// TODO: does it need to be that generic?
pub trait HandleReports<T: Config> {
	/// The offence type used for reporting offences on valid reports for disputes
	/// lost about a valid candidate.
	type OffenceForInvalid: SlashingOffence<T::KeyOwnerIdentification>;

	/// The offence type used for reporting offences on valid reports for disputes
	/// lost about an invalid candidate.
	type OffenceAgainstValid: SlashingOffence<T::KeyOwnerIdentification>;

	/// The longevity, in blocks, that the offence report is valid for. When using the staking
	/// pallet this should be equal to the bonding duration (in blocks, not eras).
	type ReportLongevity: Get<u64>;

	/// Report a for valid offence.
	// TODO: generic over reporter type?
	fn report_for_invalid_offence(
		reporters: Vec<AccountId<T>>,
		offence: Self::OffenceForInvalid,
	) -> Result<(), OffenceError>;

	/// Report an against invalid offence.
	fn report_against_valid_offence(
		reporters: Vec<AccountId<T>>,
		offence: Self::OffenceAgainstValid,
	) -> Result<(), OffenceError>;

	/// Returns true if the offenders at the given time slot has already been reported.
	fn is_known_for_invalid_offence(
		offenders: &[T::KeyOwnerIdentification],
		time_slot: &<Self::OffenceForInvalid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool;

	/// Returns true if the offenders at the given time slot has already been reported.
	fn is_known_against_valid_offence(
		offenders: &[T::KeyOwnerIdentification],
		time_slot: &<Self::OffenceAgainstValid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool;

	/// Create and dispatch a slashing report extrinsic.
	/// This should be called offchain.
	fn submit_unsigned_slashing_report(
		dispute_proof: DisputeProof,
		key_owner_proof: T::KeyOwnerProof,
	) -> DispatchResult;
}

impl<T: Config> HandleReports<T> for () {
	type OffenceForInvalid = ForInvalidOffence<T::KeyOwnerIdentification>;
	type OffenceAgainstValid = AgainstValidOffence<T::KeyOwnerIdentification>;
	type ReportLongevity = ();

	fn report_for_invalid_offence(
		_reporters: Vec<AccountId<T>>,
		_offence: Self::OffenceForInvalid,
	) -> Result<(), OffenceError> {
		Ok(())
	}

	fn report_against_valid_offence(
		_reporters: Vec<AccountId<T>>,
		_offence: Self::OffenceAgainstValid,
	) -> Result<(), OffenceError> {
		Ok(())
	}

	fn is_known_for_invalid_offence(
		_offenders: &[T::KeyOwnerIdentification],
		_time_slot: &<Self::OffenceForInvalid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool {
		true
	}

	fn is_known_against_valid_offence(
		_offenders: &[T::KeyOwnerIdentification],
		_time_slot: &<Self::OffenceAgainstValid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool {
		true
	}

	fn submit_unsigned_slashing_report(
		_dispute_proof: DisputeProof,
		_key_owner_proof: T::KeyOwnerProof,
	) -> DispatchResult {
		Ok(())
	}
}

pub trait WeightInfo {
	fn report_dispute_lost(validator_count: u32) -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn report_dispute_lost(_validator_count: u32) -> Weight {
		0
	}
}

pub use pallet::*;
#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config + crate::disputes::Config {
		// type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// The proof of key ownership, used for validating slashing reports.
		/// The proof must include the session index and validator count of the
		/// session at which the equivocation occurred.
		type KeyOwnerProof: Parameter + GetSessionNumber + GetValidatorCount;

		/// The identification of a key owner, used when reporting slashes.
		type KeyOwnerIdentification: Parameter;

		/// A system for proving ownership of keys, i.e. that a given key was part
		/// of a validator set, needed for validating slashing reports.
		type KeyOwnerProofSystem: KeyOwnerProofSystem<
			(KeyTypeId, ValidatorId),
			Proof = Self::KeyOwnerProof,
			IdentificationTuple = Self::KeyOwnerIdentification,
		>;

		/// The slashing report handling subsystem, defines methods to report an
		/// offence (after the slashing report has been validated) and for submitting a
		/// transaction to report a slash (from an offchain context).
		/// NOTE: when enabling slashing report handling (i.e. this type isn't set to
		/// `()`) you must use this pallet's `ValidateUnsigned` in the runtime
		/// definition.
		type HandleReports: HandleReports<Self>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	/// Indices of the validators pending "for invalid" dispute slashes.
	#[pallet::storage]
	pub(super) type PendingForInvalidLosers<T> =
		StorageDoubleMap<_, Twox64Concat, SessionIndex, Blake2_128Concat, CandidateHash, Losers>;

	/// Indices of the validators who won in "for invalid" disputes.
	#[pallet::storage]
	pub(super) type ForInvalidWinners<T> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		Winners<T>,
	>;

	/// Indices of the validators pending "against valid" dispute slashes.
	#[pallet::storage]
	pub(super) type PendingAgainstValidLosers<T> =
		StorageDoubleMap<_, Twox64Concat, SessionIndex, Blake2_128Concat, CandidateHash, Losers>;

	/// Indices of the validators who won in "against valid" disputes.
	#[pallet::storage]
	pub(super) type AgainstValidWinners<T> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		Winners<T>,
	>;

	#[pallet::error]
	pub enum Error<T> {
		/// The key ownership proof is invalid.
		InvalidKeyOwnershipProof,
		/// The session index is too old or invalid.
		InvalidSessionIndex,
		/// The candidate hash is invalid.
		InvalidCandidateHash,
		/// There is no pending slash for the given validator index and time slot.
		InvalidValidatorIndex,
		/// The validator index does not match the validator id.
		ValidatorIndexIdMismatch,
		/// The given slashing report is valid but already previously reported.
		DuplicateSlashingReport,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(<T as Config>::WeightInfo::report_dispute_lost(
			key_owner_proof.validator_count()
		))]
		pub fn report_dispute_lost_unsigned(
			origin: OriginFor<T>,
			// box to decrease the size of the call
			dispute_proof: Box<DisputeProof>,
			key_owner_proof: T::KeyOwnerProof,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;

			// check the membership proof to extract the offender's id
			let key = (primitives::v2::PARACHAIN_KEY_TYPE_ID, dispute_proof.validator_id.clone());
			let offender = T::KeyOwnerProofSystem::check_proof(key, key_owner_proof.clone())
				.ok_or(Error::<T>::InvalidKeyOwnershipProof)?;

			// check that `validator_index` matches `validator_id`
			let session_index = dispute_proof.time_slot.session_index;
			let validator_set_count =
				if let Some(info) = crate::session_info::Pallet::<T>::session_info(session_index) {
					let i = dispute_proof.validator_index.0 as usize;
					ensure!(
						info.validators.get(i) == Some(&dispute_proof.validator_id),
						Error::<T>::ValidatorIndexIdMismatch
					);
					// number of all validators (not only parachain) in the session
					info.discovery_keys.len() as u32
				} else {
					return Err(Error::<T>::InvalidSessionIndex.into())
				};

			// check that there is a pending slash for the given
			// validator index and candidate hash
			let candidate_hash = dispute_proof.time_slot.candidate_hash;
			let try_remove = |v: &mut Option<Losers>| -> Result<(), DispatchError> {
				let indices = v.as_mut().ok_or(Error::<T>::InvalidCandidateHash)?;

				ensure!(
					indices.remove(&dispute_proof.validator_index),
					Error::<T>::InvalidValidatorIndex,
				);

				if indices.is_empty() {
					*v = None;
				}

				Ok(())
			};
			match dispute_proof.kind {
				SlashingOffenceKind::ForInvalid => {
					<PendingForInvalidLosers<T>>::try_mutate_exists(
						&session_index,
						&candidate_hash,
						try_remove,
					)?;

					let winners = <ForInvalidWinners<T>>::get(&session_index, &candidate_hash)
						.unwrap_or_default();

					let offence = <T::HandleReports as HandleReports<T>>::OffenceForInvalid::new(
						session_index,
						candidate_hash,
						validator_set_count,
						offender,
					);

					// We can't really submit a duplicate report
					// unless there's a bug.
					<T::HandleReports as HandleReports<T>>::report_for_invalid_offence(
						winners, offence,
					)
					.map_err(|_| Error::<T>::DuplicateSlashingReport)?;
				},
				SlashingOffenceKind::AgainstValid => {
					<PendingAgainstValidLosers<T>>::try_mutate_exists(
						&session_index,
						&candidate_hash,
						try_remove,
					)?;

					let winners = <ForInvalidWinners<T>>::get(&session_index, &candidate_hash)
						.unwrap_or_default();

					// submit an offence report
					let offence = <T::HandleReports as HandleReports<T>>::OffenceAgainstValid::new(
						session_index,
						candidate_hash,
						validator_set_count,
						offender,
					);
					// We can't really submit a duplicate report
					// unless there's a bug.
					<T::HandleReports as HandleReports<T>>::report_against_valid_offence(
						winners, offence,
					)
					.map_err(|_| Error::<T>::DuplicateSlashingReport)?;
				},
			}

			Ok(Pays::No.into())
		}
	}

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;
		fn validate_unsigned(source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			Self::validate_unsigned(source, call)
		}

		fn pre_dispatch(call: &Self::Call) -> Result<(), TransactionValidityError> {
			Self::pre_dispatch(call)
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the disputes slashing module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the disputes slashing pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note a new session in the disputes slashing pallet.
	pub(crate) fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
	) {
		let config = <super::configuration::Pallet<T>>::config();
		if notification.session_index <= config.dispute_period + 1 {
			return
		}

		let old_session = notification.session_index - config.dispute_period - 1;

		// TODO: warn if there were pending slashes?
		<PendingForInvalidLosers<T>>::remove_prefix(old_session, None);
		<PendingAgainstValidLosers<T>>::remove_prefix(old_session, None);
		<ForInvalidWinners<T>>::remove_prefix(old_session, None);
		<AgainstValidWinners<T>>::remove_prefix(old_session, None);
	}
}

/// Methods for the `ValidateUnsigned` implementation:
/// It restricts calls to `report_dispute_lost_unsigned` to local calls (i.e. extrinsics generated
/// on this node) or that already in a block. This guarantees that only block authors can include
/// unsigned slashing reports.
impl<T: Config> Pallet<T> {
	pub fn validate_unsigned(source: TransactionSource, call: &Call<T>) -> TransactionValidity {
		if let Call::report_dispute_lost_unsigned { dispute_proof, key_owner_proof } = call {
			// discard slashing report not coming from the local node
			match source {
				TransactionSource::Local | TransactionSource::InBlock => { /* allowed */ },
				_ => {
					log::warn!(
						target: LOG_TARGET,
						"rejecting unsigned transaction because it is not local/in-block."
					);

					return InvalidTransaction::Call.into()
				},
			}

			// check report staleness
			is_known_offence::<T>(dispute_proof, key_owner_proof)?;

			let longevity = <T::HandleReports as HandleReports<T>>::ReportLongevity::get();

			let tag_prefix = match dispute_proof.kind {
				SlashingOffenceKind::ForInvalid => "DisputeForInvalid",
				SlashingOffenceKind::AgainstValid => "DisputeAgainstValid",
			};

			ValidTransaction::with_tag_prefix(tag_prefix)
				// We assign the maximum priority for any report.
				.priority(TransactionPriority::max_value())
				// Only one report for the same offender at the same slot.
				.and_provides((dispute_proof.time_slot.clone(), dispute_proof.validator_id.clone()))
				.longevity(longevity)
				// We don't propagate this. This can never be included on a remote node.
				.propagate(false)
				.build()
		} else {
			InvalidTransaction::Call.into()
		}
	}

	pub fn pre_dispatch(call: &Call<T>) -> Result<(), TransactionValidityError> {
		if let Call::report_dispute_lost_unsigned { dispute_proof, key_owner_proof } = call {
			is_known_offence::<T>(dispute_proof, key_owner_proof)
		} else {
			Err(InvalidTransaction::Call.into())
		}
	}
}

fn is_known_offence<T: Config>(
	dispute_proof: &DisputeProof,
	key_owner_proof: &T::KeyOwnerProof,
) -> Result<(), TransactionValidityError> {
	// check the membership proof to extract the offender's id
	let key = (primitives::v2::PARACHAIN_KEY_TYPE_ID, dispute_proof.validator_id.clone());

	let offender = T::KeyOwnerProofSystem::check_proof(key, key_owner_proof.clone())
		.ok_or(InvalidTransaction::BadProof)?;

	// check if the offence has already been reported,
	// and if so then we can discard the report.
	let is_known_offence = match dispute_proof.kind {
		SlashingOffenceKind::ForInvalid => {
			let time_slot =
				<T::HandleReports as HandleReports<T>>::OffenceForInvalid::new_time_slot(
					dispute_proof.time_slot.session_index,
					dispute_proof.time_slot.candidate_hash,
				);

			<T::HandleReports as HandleReports<T>>::is_known_for_invalid_offence(
				&[offender],
				&time_slot,
			)
		},
		SlashingOffenceKind::AgainstValid => {
			let time_slot =
				<T::HandleReports as HandleReports<T>>::OffenceAgainstValid::new_time_slot(
					dispute_proof.time_slot.session_index,
					dispute_proof.time_slot.candidate_hash,
				);

			<T::HandleReports as HandleReports<T>>::is_known_against_valid_offence(
				&[offender],
				&time_slot,
			)
		},
	};

	if is_known_offence {
		Err(InvalidTransaction::Stale.into())
	} else {
		Ok(())
	}
}

// TODO: docs
pub struct SlashingReportHandler<I, R, L> {
	_phantom: sp_std::marker::PhantomData<(I, R, L)>,
}

impl<I, R, L> Default for SlashingReportHandler<I, R, L> {
	fn default() -> Self {
		Self { _phantom: Default::default() }
	}
}

impl<T, R, L> HandleReports<T> for SlashingReportHandler<T::KeyOwnerIdentification, R, L>
where
	T: Config + frame_system::offchain::SendTransactionTypes<Call<T>>,
	R: ReportOffence<
			AccountId<T>,
			T::KeyOwnerIdentification,
			ForInvalidOffence<T::KeyOwnerIdentification>,
		> + ReportOffence<
			AccountId<T>,
			T::KeyOwnerIdentification,
			AgainstValidOffence<T::KeyOwnerIdentification>,
		>,
	L: Get<u64>,
{
	type OffenceForInvalid = ForInvalidOffence<T::KeyOwnerIdentification>;

	type OffenceAgainstValid = AgainstValidOffence<T::KeyOwnerIdentification>;

	type ReportLongevity = L;

	fn report_for_invalid_offence(
		reporters: Vec<AccountId<T>>,
		offence: Self::OffenceForInvalid,
	) -> Result<(), OffenceError> {
		R::report_offence(reporters, offence)
	}

	fn report_against_valid_offence(
		reporters: Vec<AccountId<T>>,
		offence: Self::OffenceAgainstValid,
	) -> Result<(), OffenceError> {
		R::report_offence(reporters, offence)
	}

	fn is_known_for_invalid_offence(
		offenders: &[T::KeyOwnerIdentification],
		time_slot: &DisputesTimeSlot,
	) -> bool {
		<R as ReportOffence<
			AccountId<T>,
			T::KeyOwnerIdentification,
			ForInvalidOffence<T::KeyOwnerIdentification>,
		>>::is_known_offence(offenders, time_slot)
	}

	fn is_known_against_valid_offence(
		offenders: &[T::KeyOwnerIdentification],
		time_slot: &DisputesTimeSlot,
	) -> bool {
		<R as ReportOffence<
			AccountId<T>,
			T::KeyOwnerIdentification,
			AgainstValidOffence<T::KeyOwnerIdentification>,
		>>::is_known_offence(offenders, time_slot)
	}

	fn submit_unsigned_slashing_report(
		dispute_proof: DisputeProof,
		key_owner_proof: <T as Config>::KeyOwnerProof,
	) -> DispatchResult {
		use frame_system::offchain::SubmitTransaction;

		let session_index = dispute_proof.time_slot.session_index;
		let validator_index = dispute_proof.validator_index.0;
		let kind = dispute_proof.kind;

		let call = Call::report_dispute_lost_unsigned {
			dispute_proof: Box::new(dispute_proof),
			key_owner_proof,
		};

		match SubmitTransaction::<T, Call<T>>::submit_unsigned_transaction(call.into()) {
			Ok(()) => log::info!(
				target: LOG_TARGET,
				"Submitted dispute slashing report, session: {}, validator: {}, kind: {:?}",
				session_index,
				validator_index,
				kind,
			),
			Err(()) => log::error!(
				target: LOG_TARGET,
				"Error submitting dispute slashing report, session: {}, validator: {}, kind: {:?}",
				session_index,
				validator_index,
				kind,
			),
		}

		Ok(())
	}
}
