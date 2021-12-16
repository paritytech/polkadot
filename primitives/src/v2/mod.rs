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

//! `V2` Primitives.


pub use crate::v1::*;

use parity_scale_codec::{Decode, Encode};
use primitives::RuntimeDebug;
use scale_info::TypeInfo;

#[cfg(feature = "std")]
use parity_util_mem::MallocSizeOf;

/// Information about validator sets of a session.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, MallocSizeOf))]
pub struct SessionInfo {
	/****** New in v2 *******/

	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	pub active_validator_indices: Vec<ValidatorIndex>,



	/****** Old fields ******/

	/// Validators in canonical ordering.
	///
	/// NOTE: There might be more authorities in the current session, than `validators` participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// `SessionInfo::validators` will be limited to to `max_validators` when set.
	pub validators: Vec<ValidatorId>,
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
	pub validator_groups: Vec<Vec<ValidatorIndex>>,
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
