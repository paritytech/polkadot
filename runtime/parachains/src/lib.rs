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

//! Runtime modules for parachains code.
//!
//! It is crucial to include all the modules from this crate in the runtime, in
//! particular the `Initializer` module, as it is responsible for initializing the state
//! of the other modules.

mod configuration;
mod inclusion;
mod inclusion_inherent;
mod initializer;
mod paras;
mod scheduler;
mod validity;

#[cfg(test)]
mod mock;

/// A module exporting runtime API implementation functions for all runtime APIs using v1
/// primitives.
///
/// Runtimes implementing the v1 runtime API are recommended to forward directly to these
/// functions.
pub mod runtime_api_impl_v1 {
	use primitives::v1::{
		ValidatorId, ValidatorIndex, GroupRotationInfo, CoreState, GlobalValidationSchedule,
		Id as ParaId, OccupiedCoreAssumption, LocalValidationData, SessionIndex, ValidationCode,
		CommittedCandidateReceipt,
	};
	use crate::initializer;

	/// Implementation for the `validators` function of the runtime API.
	pub fn validators<T: initializer::Trait>() -> Vec<ValidatorId> {
		unimplemented!()
	}

	/// Implementation for the `validator_groups` function of the runtime API.
	pub fn validator_groups<T: initializer::Trait>() -> (
		Vec<Vec<ValidatorIndex>>,
		GroupRotationInfo,
	) {
		unimplemented!()
	}

	/// Implementation for the `availability_cores` function of the runtime API.
	pub fn availability_cores<T: initializer::Trait>() -> Vec<CoreState> {
		unimplemented!()
	}

	/// Implementation for the `global_validation_schedule` function of the runtime API.
	pub fn global_validation_schedule<T: initializer::Trait>()
		-> GlobalValidationSchedule
	{
		unimplemented!()
	}

	/// Implementation for the `local_validation_data` function of the runtime API.
	pub fn local_validation_data<T: initializer::Trait>(
		para: ParaId,
		assumption: OccupiedCoreAssumption,
	) -> Option<LocalValidationData> {
		unimplemented!()
	}

	/// Implementation for the `session_index_for_child` function of the runtime API.
	pub fn session_index_for_child<T: initializer::Trait>() -> SessionIndex {
		unimplemented!()
	}

	/// Implementation for the `validation_code` function of the runtime API.
	pub fn validation_code<T: initializer::Trait>(
		para: ParaId,
		assumption: OccupiedCoreAssumption,
	) -> Option<ValidationCode> {
		unimplemented!()
	}

	/// Implementation for the `candidate_pending_availability` function of the runtime API.
	pub fn candidate_pending_availability<T: initializer::Trait>(para: ParaId)
		-> Option<CommittedCandidateReceipt>
	{
		unimplemented!()
	}
}
