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

use super::{Config, IdentificationTuple, IdentifyValidatorsInSession};
use parity_scale_codec::{Decode, Encode};
use primitives::v2::{CandidateHash, SessionIndex, ValidatorIndex};
use scale_info::TypeInfo;
use sp_runtime::{Perbill, RuntimeDebug};
use sp_staking::offence::{DisableStrategy, Kind, Offence, ReportOffence};
use sp_std::prelude::*;

// TODO: docs
// timeslots should uniquely identify offences
// and are used for deduplication
// OTOH, we won't submit duplicate offences
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub struct DisputesTimeSlot<BlockNumber = primitives::v2::BlockNumber> {
	// TODO: `TimeSlot` docs says it should fit into `u128`
	// any proofs?

	// ordering is important for timeslots
	// why?
	dispute_start_block_number: BlockNumber,
	candidate_hash: CandidateHash,
}

impl<BlockNumber> DisputesTimeSlot<BlockNumber> {
	pub fn new(dispute_start_block_number: BlockNumber, candidate_hash: CandidateHash) -> Self {
		Self { dispute_start_block_number, candidate_hash }
	}
}

// TODO design: one offence type vs multiple

/// An offence that is filed when a series of validators lost a dispute
/// about an invalid candidate.
#[derive(RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct ForInvalidOffence<BlockNumber, FullIdentification> {
	/// The session index when the candidate was backed/included.
	pub session_index: SessionIndex,
	/// The size of the validator set in that session.
	/// Note: this included not only parachain validators.
	pub validator_set_count: u32,
	/// Should be unique per dispute.
	pub time_slot: DisputesTimeSlot<BlockNumber>,
	/// Stash keys of the validators that lost the dispute
	/// and their nominators.
	pub offenders: Vec<FullIdentification>,
}

impl<BlockNumber, Offender> Offence<Offender> for ForInvalidOffence<BlockNumber, Offender>
where
	BlockNumber: Encode + Decode + Ord + Clone,
	Offender: Clone,
{
	const ID: Kind = *b"disputes:invalid";

	type TimeSlot = DisputesTimeSlot<BlockNumber>;

	fn offenders(&self) -> Vec<Offender> {
		self.offenders.clone()
	}

	fn session_index(&self) -> SessionIndex {
		self.session_index
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
		Perbill::from_percent(100)
	}
}

// TODO: this can include a multiplier to make slashing worse
// and enable disabling
/// An offence that is filed when a series of validators lost a dispute
/// about an valid candidate.
#[derive(RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct AgainstValidOffence<BlockNumber, FullIdentification> {
	/// The session index when the candidate was backed/included.
	pub session_index: SessionIndex,
	/// The size of the validator set in that session.
	/// Note: this included not only parachain validators.
	pub validator_set_count: u32,
	/// Should be unique per dispute.
	pub time_slot: DisputesTimeSlot<BlockNumber>,
	/// Stash keys of the validators that lost the dispute
	/// and their nominators.
	pub offenders: Vec<FullIdentification>,
}

impl<BlockNumber, Offender> Offence<Offender> for AgainstValidOffence<BlockNumber, Offender>
where
	BlockNumber: Encode + Decode + Ord + Clone,
	Offender: Clone,
{
	const ID: Kind = *b"disputes:valid::";

	type TimeSlot = DisputesTimeSlot<BlockNumber>;

	fn offenders(&self) -> Vec<Offender> {
		self.offenders.clone()
	}

	fn session_index(&self) -> SessionIndex {
		self.session_index
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
pub struct SlashValidatorsForDisputes<C> {
	_phantom: sp_std::marker::PhantomData<C>,
}

impl<C> Default for SlashValidatorsForDisputes<C> {
	fn default() -> Self {
		Self { _phantom: Default::default() }
	}
}

impl<T> super::PunishValidators<T::BlockNumber> for SlashValidatorsForDisputes<T>
where
	T: Config,
{
	fn punish_for_invalid(
		session_index: SessionIndex,
		block_number: T::BlockNumber,
		candidate_hash: CandidateHash,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		let reporters = Vec::new(); // TODO pass winners
		let time_slot =
			DisputesTimeSlot { dispute_start_block_number: block_number, candidate_hash };
		let session_info = match <crate::session_info::Pallet<T>>::session_info(session_index) {
			Some(session_info) => session_info,
			None => return,
		};
		let validator_set_count = session_info.discovery_keys.len() as u32;
		let offenders = <
			<T as Config>::IdentifyValidatorsInSession as IdentifyValidatorsInSession
		>::identify_validators_in_session(session_index, validators);

		let offence =
			ForInvalidOffence { session_index, offenders, time_slot, validator_set_count };
		// TODO: how to handle errors?
		let _ = <<T as Config>::ReportDisputeOffences as ReportOffence<
			T::AccountId,
			IdentificationTuple<T>,
			ForInvalidOffence<T::BlockNumber, IdentificationTuple<T>>,
		>>::report_offence(reporters, offence);
	}

	fn punish_against_valid(
		session_index: SessionIndex,
		block_number: T::BlockNumber,
		candidate_hash: CandidateHash,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		let time_slot =
			DisputesTimeSlot { dispute_start_block_number: block_number, candidate_hash };
		let session_info = match <crate::session_info::Pallet<T>>::session_info(session_index) {
			Some(session_info) => session_info,
			None => return,
		};
		let validator_set_count = session_info.discovery_keys.len() as u32;
		let offenders = <
			<T as Config>::IdentifyValidatorsInSession as IdentifyValidatorsInSession
		>::identify_validators_in_session(session_index, validators);

		let offence =
			AgainstValidOffence { session_index, offenders, time_slot, validator_set_count };
		let reporters = Vec::new(); // TODO pass winners
		let _ = <<T as Config>::ReportDisputeOffences as ReportOffence<
			T::AccountId,
			IdentificationTuple<T>,
			AgainstValidOffence<T::BlockNumber, IdentificationTuple<T>>,
		>>::report_offence(reporters, offence);
	}

	fn punish_inconclusive(
		_session_index: SessionIndex,
		_block_number: T::BlockNumber,
		_candidate_hash: CandidateHash,
		_validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		// TODO
	}
}
