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

// use pallet_session::KeyOwner;
// use super::{Config, IdentificationTuple, IdentifyValidatorsInSession};
use crate::initializer::SessionChangeNotification;
use frame_support::{
	traits::{Get, KeyOwnerProofSystem, ValidatorSet, ValidatorSetWithIdentification},
	weights::{Pays, Weight},
};
use parity_scale_codec::{Decode, Encode};
use primitives::v2::{CandidateHash, SessionIndex, ValidatorId, ValidatorIndex};
use scale_info::TypeInfo;
use sp_runtime::{traits::Convert, DispatchResult, KeyTypeId, Perbill, RuntimeDebug};
use sp_session::{GetSessionNumber, GetValidatorCount};
use sp_staking::offence::{DisableStrategy, Kind, Offence, OffenceError, ReportOffence};
use sp_std::{collections::btree_set::BTreeSet, prelude::*};

// TODO: docs
// timeslots should uniquely identify offences
// and are used for deduplication
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub struct DisputesTimeSlot {
	// TODO: `TimeSlot` docs says it should fit into `u128`
	// any proofs?

	// ordering is important for timeslots
	// why?
	session_index: SessionIndex,
	candidate_hash: CandidateHash,
}

impl DisputesTimeSlot {
	pub fn new(session_index: SessionIndex, candidate_hash: CandidateHash) -> Self {
		Self { session_index, candidate_hash }
	}
}

// TODO design: one offence type vs multiple

/// An offence that is filed when a series of validators lost a dispute
/// about an invalid candidate.
#[derive(RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct ForInvalidOffence<KeyOwnerIdentification> {
	/// The size of the validator set in that session.
	/// Note: this included not only parachain validators.
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
		Perbill::from_percent(100) // ggez
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
	/// Note: this included not only parachain validators.
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

/// A type for representing the validator account id in a session.
type AccountId<T> = <<T as Config>::ValidatorSet as ValidatorSet<
	<T as frame_system::Config>::AccountId,
>>::ValidatorId;

/// A tuple of `(AccountId, Identification)` where `Identification`
/// is the full identification of `AccountId`.
pub type IdentificationTuple<T> = (
	AccountId<T>,
	<<T as Config>::ValidatorSet as ValidatorSetWithIdentification<
		<T as frame_system::Config>::AccountId,
	>>::Identification,
);

impl<T, R> super::PunishValidators for SlashValidatorsForDisputes<T, R>
where
	T: Config + crate::shared::Config,
	R: ReportOffence<
			T::AccountId,
			IdentificationTuple<T>,
			ForInvalidOffence<IdentificationTuple<T>>,
		> + ReportOffence<
			T::AccountId,
			IdentificationTuple<T>,
			AgainstValidOffence<IdentificationTuple<T>>,
		>,
{
	fn punish_for_invalid(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		// TODO: make sure we're handling off-by-one correctly
		// TODO: set reporters to the winning side of the dispute
		let current_session = <<T as Config>::ValidatorSet>::session_index();
		if session_index == current_session {
			if let Some(account_ids) = <AccountIds<T>>::get(session_index) {
				let reporters = vec![];
				let validator_set_count = account_ids.len() as u32;
				let time_slot = DisputesTimeSlot::new(session_index, candidate_hash);
				let shuffled_indices = <crate::shared::Pallet<T>>::active_validator_indices();
				let offenders = validators
					.into_iter()
					.flat_map(|i| shuffled_indices.get(i.0 as usize).cloned())
					.flat_map(|i| account_ids.get(i.0 as usize).cloned())
					.filter_map(|id| {
						<T::ValidatorSet as ValidatorSetWithIdentification<T::AccountId>>::IdentificationOf::convert(
							id.clone()
						).map(|full_id| (id, full_id))
					})
					.collect::<Vec<IdentificationTuple<T>>>();
				let offence = ForInvalidOffence { validator_set_count, time_slot, offenders };
				// TODO: log error?
				let _ = R::report_offence(reporters, offence);
				return
			}
		}
		let validators: BTreeSet<ValidatorIndex> = validators.into_iter().collect();
		<PendingSlashesForInvalid<T>>::insert(session_index, candidate_hash, validators);
	}

	fn punish_against_valid(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		// TODO: make sure we're handling off-by-one correctly
		// TODO: set reporters to the winning side of the dispute
		// TODO: deduplicate the code
		let current_session = <<T as Config>::ValidatorSet>::session_index();
		if session_index == current_session {
			if let Some(account_ids) = <AccountIds<T>>::get(session_index) {
				let reporters = vec![];
				let validator_set_count = account_ids.len() as u32;
				let time_slot = DisputesTimeSlot::new(session_index, candidate_hash);
				let shuffled_indices = <crate::shared::Pallet<T>>::active_validator_indices();
				let offenders = validators
					.into_iter()
					.flat_map(|i| shuffled_indices.get(i.0 as usize).cloned())
					.flat_map(|i| account_ids.get(i.0 as usize).cloned())
					.filter_map(|id| {
						<T::ValidatorSet as ValidatorSetWithIdentification<T::AccountId>>::IdentificationOf::convert(
							id.clone()
						).map(|full_id| (id, full_id))
					})
					.collect::<Vec<IdentificationTuple<T>>>();
				let offence = AgainstValidOffence { validator_set_count, time_slot, offenders };
				// TODO: log error?
				let _ = R::report_offence(reporters, offence);
				return
			}
		}
		let validators: BTreeSet<ValidatorIndex> = validators.into_iter().collect();
		<PendingSlashesAgainstValid<T>>::insert(session_index, candidate_hash, validators);
	}

	fn punish_inconclusive(
		_session_index: SessionIndex,
		_candidate_hash: CandidateHash,
		_validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		// TODO
	}
}

#[derive(PartialEq, Eq, Clone, Copy, Encode, Decode, RuntimeDebug, TypeInfo)]
pub enum SlashingOffenceKind {
	#[codec(index = 0)]
	ForInvalid,
	#[codec(index = 1)]
	AgainstValid,
}

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

pub trait HandleSlashingReportsForOldSessions<T: Config> {
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
	/// TODO: do we want to pass reporters or derive from the dispute?
	fn report_for_invalid_offence(offence: Self::OffenceForInvalid) -> Result<(), OffenceError>;

	/// Report an against invalid offence.
	fn report_against_valid_offence(offence: Self::OffenceAgainstValid)
		-> Result<(), OffenceError>;

	/// Returns true if the offender at the given time slot has already been reported.
	fn is_known_for_invalid_offence(
		offenders: &T::KeyOwnerIdentification,
		time_slot: &<Self::OffenceForInvalid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool;

	/// Returns true if the offender at the given time slot has already been reported.
	fn is_known_against_valid_offence(
		offenders: &T::KeyOwnerIdentification,
		time_slot: &<Self::OffenceAgainstValid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool;

	/// Create and dispatch a slashing report extrinsic.
	fn submit_unsigned_slashing_report(
		session_index: SessionIndex,
		candidate_hash: CandidateHash,
		kind: SlashingOffenceKind,
		key_owner_proof: T::KeyOwnerProof,
	) -> DispatchResult;
}

impl<T: Config> HandleSlashingReportsForOldSessions<T> for () {
	type OffenceForInvalid = ForInvalidOffence<T::KeyOwnerIdentification>;
	type OffenceAgainstValid = AgainstValidOffence<T::KeyOwnerIdentification>;
	type ReportLongevity = ();

	fn report_for_invalid_offence(_offence: Self::OffenceForInvalid) -> Result<(), OffenceError> {
		Ok(())
	}

	fn report_against_valid_offence(
		_offence: Self::OffenceAgainstValid,
	) -> Result<(), OffenceError> {
		Ok(())
	}

	fn is_known_for_invalid_offence(
		_offenders: &T::KeyOwnerIdentification,
		_time_slot: &<Self::OffenceForInvalid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool {
		true
	}

	fn is_known_against_valid_offence(
		_offenders: &T::KeyOwnerIdentification,
		_time_slot: &<Self::OffenceAgainstValid as Offence<T::KeyOwnerIdentification>>::TimeSlot,
	) -> bool {
		true
	}

	fn submit_unsigned_slashing_report(
		_session_index: SessionIndex,
		_candidate_hash: CandidateHash,
		_kind: SlashingOffenceKind,
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

		/// A type that is helpful for submitting offences for the current session disputes.
		// TODO: make sure we're handling correctly the session index one-off on the boundaries
		// between the parachains and session modules
		type ValidatorSet: ValidatorSetWithIdentification<Self::AccountId>;

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
		type HandleSlashingReportsForOldSessions: HandleSlashingReportsForOldSessions<Self>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	/// Pending "for invalid" dispute slashes for the last several sessions.
	#[pallet::storage]
	pub(super) type PendingSlashesForInvalid<T> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		BTreeSet<ValidatorIndex>, // TODO: also store winners
	>;

	/// Pending "against valid" dispute slashes for the last several sessions.
	#[pallet::storage]
	pub(super) type PendingSlashesAgainstValid<T> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		BTreeSet<ValidatorIndex>, // TODO separate backing from approval and dispute votes?
	>;

	/// `AccountId`s of the validators in the canonical ordering for the last several sessions.
	// TODO: clarify canonical ordering
	#[pallet::storage]
	pub(super) type AccountIds<T> = StorageMap<_, Twox64Concat, SessionIndex, Vec<AccountId<T>>>;

	#[pallet::error]
	pub enum Error<T> {
		/// A key ownership proof provided as part of an slashing report is invalid.
		InvalidKeyOwnershipProof,
		/// The session index provided as part of an slashing report is invalid.
		InvalidSessionIndex,
		/// The candidate hash provided as part of an slashing report is invalid.
		InvalidCandidateHash,
		/// A given slashing report is valid but already previously reported.
		DuplicateSlashingReport,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(<T as Config>::WeightInfo::report_dispute_lost(
			_key_owner_proof.validator_count()
		))]
		pub fn report_dispute_lost_unsigned(
			origin: OriginFor<T>,
			_session_index: SessionIndex,
			_candidate_hash: CandidateHash,
			_kind: SlashingOffenceKind,
			_key_owner_proof: T::KeyOwnerProof,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;
			// TODO: impl
			Ok(Pays::No.into())
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
		// FIXME: off-by-one on boundaries?
		let account_ids = T::ValidatorSet::validators();
		<AccountIds<T>>::insert(notification.session_index, account_ids);

		let config = <super::configuration::Pallet<T>>::config();
		if notification.session_index <= config.dispute_period + 1 {
			return
		}

		// TODO: we could keep em longer than dispute_period?
		let pruning_target = notification.session_index - config.dispute_period - 1;
		let last_pruned = <crate::disputes::LastPrunedSession<T>>::get();
		let to_prune = if let Some(last_pruned) = last_pruned {
			last_pruned + 1..=pruning_target
		} else {
			pruning_target..=pruning_target
		};

		for old_session in to_prune {
			<PendingSlashesForInvalid<T>>::remove_prefix(old_session, None);
			<PendingSlashesAgainstValid<T>>::remove_prefix(old_session, None);
			<AccountIds<T>>::remove(old_session);
		}
	}
}

// TODO: how to avoid this abomination?
pub struct MockValidatorSet;

impl<AccoundId> ValidatorSet<AccoundId> for MockValidatorSet {
	type ValidatorId = ();
	type ValidatorIdOf = ();
	fn session_index() -> SessionIndex {
		0
	}
	fn validators() -> Vec<Self::ValidatorId> {
		Vec::new()
	}
}

impl<AccountId> ValidatorSetWithIdentification<AccountId> for MockValidatorSet {
	type Identification = ();
	type IdentificationOf = ();
}
