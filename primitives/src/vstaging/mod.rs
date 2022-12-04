// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

//! Staging Primitives.

// Put any primitives used by staging APIs functions here

pub mod executor_params;
pub use executor_params::{
	ExecInstantiationStrategy, ExecutionEnvironment, ExecutorParam, ExecutorParams,
	ExecutorParamsHash,
};

use crate::v2::{
	self, AssignmentId, AuthorityDiscoveryId, GroupIndex, IndexedVec, SessionIndex, ValidatorId,
	ValidatorIndex,
};
use parity_scale_codec::{Decode, Encode};
#[cfg(feature = "std")]
use parity_util_mem::MallocSizeOf;
use primitives::RuntimeDebug;
use scale_info::TypeInfo;
use sp_std::vec::Vec;

/// Information about validator sets of a session.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, MallocSizeOf))]
pub struct SessionInfo {
	/****** New in vstaging *******/
	/// Executor parameter set for the session.
	pub executor_params: ExecutorParams,

	/****** New in v2 *******/
	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	pub active_validator_indices: Vec<ValidatorIndex>,
	/// A secure random seed for the session, gathered from BABE.
	pub random_seed: [u8; 32],
	/// The amount of sessions to keep for disputes.
	pub dispute_period: SessionIndex,

	/****** Old fields ******/
	/// Validators in canonical ordering.
	///
	/// NOTE: There might be more authorities in the current session, than `validators` participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// `SessionInfo::validators` will be limited to to `max_validators` when set.
	pub validators: IndexedVec<ValidatorIndex, ValidatorId>,
	/// Validators' authority discovery keys for the session in canonical ordering.
	///
	/// NOTE: The first `validators.len()` entries will match the corresponding validators in
	/// `validators`, afterwards any remaining authorities can be found. This is any authorities not
	/// participating in parachain consensus - see
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148)
	#[cfg_attr(feature = "std", ignore_malloc_size_of = "outside type")]
	pub discovery_keys: Vec<AuthorityDiscoveryId>,
	/// The assignment keys for validators.
	///
	/// NOTE: There might be more authorities in the current session, than validators participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// Therefore:
	/// ```ignore
	///		assignment_keys.len() == validators.len() && validators.len() <= discovery_keys.len()
	///	```
	pub assignment_keys: Vec<AssignmentId>,
	/// Validators in shuffled ordering - these are the validator groups as produced
	/// by the `Scheduler` module for the session and are typically referred to by
	/// `GroupIndex`.
	pub validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	/// The number of availability cores used by the protocol during this session.
	pub n_cores: u32,
	/// The zeroth delay tranche width.
	pub zeroth_delay_tranche_width: u32,
	/// The number of samples we do of `relay_vrf_modulo`.
	pub relay_vrf_modulo_samples: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// How many slots (BABE / SASSAFRAS) must pass before an assignment is considered a
	/// no-show.
	pub no_show_slots: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
}

// Structure downgrade for backward compatibility
impl From<SessionInfo> for v2::SessionInfo {
	fn from(new: SessionInfo) -> Self {
		Self {
			active_validator_indices: new.active_validator_indices,
			random_seed: new.random_seed,
			dispute_period: new.dispute_period,
			validators: new.validators,
			discovery_keys: new.discovery_keys,
			assignment_keys: new.assignment_keys,
			validator_groups: new.validator_groups,
			n_cores: new.n_cores,
			zeroth_delay_tranche_width: new.zeroth_delay_tranche_width,
			relay_vrf_modulo_samples: new.relay_vrf_modulo_samples,
			n_delay_tranches: new.n_delay_tranches,
			no_show_slots: new.no_show_slots,
			needed_approvals: new.needed_approvals,
		}
	}
}

// Structure upgrade for storage translation
impl From<v2::SessionInfo> for SessionInfo {
	fn from(old: v2::SessionInfo) -> Self {
		Self {
			executor_params: ExecutorParams::default(),
			active_validator_indices: old.active_validator_indices,
			random_seed: old.random_seed,
			dispute_period: old.dispute_period,
			validators: old.validators,
			discovery_keys: old.discovery_keys,
			assignment_keys: old.assignment_keys,
			validator_groups: old.validator_groups,
			n_cores: old.n_cores,
			zeroth_delay_tranche_width: old.zeroth_delay_tranche_width,
			relay_vrf_modulo_samples: old.relay_vrf_modulo_samples,
			n_delay_tranches: old.n_delay_tranches,
			no_show_slots: old.no_show_slots,
			needed_approvals: old.needed_approvals,
		}
	}
}

// Old V1 session support
impl From<v2::OldV1SessionInfo> for SessionInfo {
	fn from(old: v2::OldV1SessionInfo) -> Self {
		v2::SessionInfo::from(old).into()
	}
}
