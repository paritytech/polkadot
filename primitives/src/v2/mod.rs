// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

use crate::v3::{AssignmentId, GroupIndex, IndexedVec, ValidatorId, ValidatorIndex};

use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_std::prelude::*;

use primitives::RuntimeDebug;

pub use runtime_primitives::traits::{BlakeTwo256, Hash as HashT};

// Export some core primitives.
pub use polkadot_core_primitives::v2::{
	AccountId, AccountIndex, AccountPublic, Balance, Block, BlockId, BlockNumber, CandidateHash,
	ChainId, DownwardMessage, Hash, Header, InboundDownwardMessage, InboundHrmpMessage, Moment,
	Nonce, OutboundHrmpMessage, Remark, Signature, UncheckedExtrinsic,
};

// Export some polkadot-parachain primitives
pub use polkadot_parachain::primitives::{
	HeadData, HrmpChannelId, Id, UpwardMessage, ValidationCode, ValidationCodeHash,
	LOWEST_PUBLIC_ID, LOWEST_USER_ID,
};

pub use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
pub use sp_consensus_slots::Slot;
pub use sp_staking::SessionIndex;

/// Information about validator sets of a session.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct SessionInfo {
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

/// Old, v1-style info about session info. Only needed for limited
/// backwards-compatibility.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct OldV1SessionInfo {
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

impl From<OldV1SessionInfo> for SessionInfo {
	fn from(old: OldV1SessionInfo) -> SessionInfo {
		SessionInfo {
			// new fields
			active_validator_indices: Vec::new(),
			random_seed: [0u8; 32],
			dispute_period: 6,
			// old fields
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
