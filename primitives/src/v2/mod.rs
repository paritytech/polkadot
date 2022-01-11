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

use crate::v1;

use parity_scale_codec::{Decode, Encode};
use primitives::RuntimeDebug;
use scale_info::TypeInfo;
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

#[cfg(feature = "std")]
use parity_util_mem::MallocSizeOf;

/// Information about validator sets of a session.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, MallocSizeOf))]
pub struct SessionInfo {
	/****** New in v2 *******/
	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	pub active_validator_indices: Vec<v1::ValidatorIndex>,
	/// A secure random seed for the session, gathered from BABE.
	pub random_seed: [u8; 32],
	/// The amount of sessions to keep for disputes.
	pub dispute_period: v1::SessionIndex,

	/****** Old fields ******/
	/// Validators in canonical ordering.
	///
	/// NOTE: There might be more authorities in the current session, than `validators` participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// `SessionInfo::validators` will be limited to to `max_validators` when set.
	pub validators: Vec<v1::ValidatorId>,
	/// Validators' authority discovery keys for the session in canonical ordering.
	///
	/// NOTE: The first `validators.len()` entries will match the corresponding validators in
	/// `validators`, afterwards any remaining authorities can be found. This is any authorities not
	/// participating in parachain consensus - see
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148)
	#[cfg_attr(feature = "std", ignore_malloc_size_of = "outside type")]
	pub discovery_keys: Vec<v1::AuthorityDiscoveryId>,
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
	pub assignment_keys: Vec<v1::AssignmentId>,
	/// Validators in shuffled ordering - these are the validator groups as produced
	/// by the `Scheduler` module for the session and are typically referred to by
	/// `GroupIndex`.
	pub validator_groups: Vec<Vec<v1::ValidatorIndex>>,
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

impl From<v1::SessionInfo> for SessionInfo {
	fn from(old: v1::SessionInfo) -> SessionInfo {
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

/// A statement from the specified validator whether the given validation code passes PVF
/// pre-checking or not anchored to the given session index.
#[derive(Encode, Decode, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct PvfCheckStatement {
	/// `true` if the subject passed pre-checking and `false` otherwise.
	pub accept: bool,
	/// The validation code hash that was checked.
	pub subject: v1::ValidationCodeHash,
	/// The index of a session during which this statement is considered valid.
	pub session_index: v1::SessionIndex,
	/// The index of the validator from which this statement originates.
	pub validator_index: v1::ValidatorIndex,
}

impl PvfCheckStatement {
	/// Produce the payload used for signing this type of statement.
	///
	/// It is expected that it will be signed by the validator at `validator_index` in the
	/// `session_index`.
	pub fn signing_payload(&self) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"VCPC"; // for "validation code pre-checking"
		(MAGIC, self.accept, self.subject, self.session_index, self.validator_index).encode()
	}
}

sp_api::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	#[api_version(2)]
	pub trait ParachainHost<H: Encode + Decode = v1::Hash, N: Encode + Decode = v1::BlockNumber> {
		/// Get the current validators.
		fn validators() -> Vec<v1::ValidatorId>;

		/// Returns the validator groups and rotation info localized based on the hypothetical child
		///  of a block whose state  this is invoked on. Note that `now` in the `GroupRotationInfo`
		/// should be the successor of the number of the block.
		fn validator_groups() -> (Vec<Vec<v1::ValidatorIndex>>, v1::GroupRotationInfo<N>);

		/// Yields information on all availability cores as relevant to the child block.
		/// Cores are either free or occupied. Free cores can have paras assigned to them.
		fn availability_cores() -> Vec<v1::CoreState<H, N>>;

		/// Yields the persisted validation data for the given `ParaId` along with an assumption that
		/// should be used if the para currently occupies a core.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		fn persisted_validation_data(para_id: v1::Id, assumption: v1::OccupiedCoreAssumption)
			-> Option<v1::PersistedValidationData<H, N>>;

		/// Returns the persisted validation data for the given `ParaId` along with the corresponding
		/// validation code hash. Instead of accepting assumption about the para, matches the validation
		/// data hash against an expected one and yields `None` if they're not equal.
		fn assumed_validation_data(
			para_id: v1::Id,
			expected_persisted_validation_data_hash: v1::Hash,
		) -> Option<(v1::PersistedValidationData<H, N>, v1::ValidationCodeHash)>;

		/// Checks if the given validation outputs pass the acceptance criteria.
		fn check_validation_outputs(para_id: v1::Id, outputs: v1::CandidateCommitments) -> bool;

		/// Returns the session index expected at a child of the block.
		///
		/// This can be used to instantiate a `SigningContext`.
		fn session_index_for_child() -> v1::SessionIndex;

		/// Old method to fetch v1 session info.
		#[changed_in(2)]
		fn session_info(index: v1::SessionIndex) -> Option<v1::SessionInfo>;

		/// Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		fn validation_code(para_id: v1::Id, assumption: v1::OccupiedCoreAssumption)
			-> Option<v1::ValidationCode>;

		/// Get the receipt of a candidate pending availability. This returns `Some` for any paras
		/// assigned to occupied cores in `availability_cores` and `None` otherwise.
		fn candidate_pending_availability(para_id: v1::Id) -> Option<v1::CommittedCandidateReceipt<H>>;

		/// Get a vector of events concerning candidates that occurred within a block.
		fn candidate_events() -> Vec<v1::CandidateEvent<H>>;

		/// Get all the pending inbound messages in the downward message queue for a para.
		fn dmq_contents(
			recipient: v1::Id,
		) -> Vec<v1::InboundDownwardMessage<N>>;

		/// Get the contents of all channels addressed to the given recipient. Channels that have no
		/// messages in them are also included.
		fn inbound_hrmp_channels_contents(recipient: v1::Id) -> BTreeMap<v1::Id, Vec<v1::InboundHrmpMessage<N>>>;

		/// Get the validation code from its hash.
		fn validation_code_by_hash(hash: v1::ValidationCodeHash) -> Option<v1::ValidationCode>;

		/// Scrape dispute relevant from on-chain, backing votes and resolved disputes.
		fn on_chain_votes() -> Option<v1::ScrapedOnChainVotes<H>>;

		/***** Added in v2 *****/

		/// Get the session info for the given session, if stored.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn session_info(index: v1::SessionIndex) -> Option<SessionInfo>;

		/// Submits a PVF pre-checking statement into the transaction pool.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn submit_pvf_check_statement(stmt: PvfCheckStatement, signature: v1::ValidatorSignature);

		/// Returns code hashes of PVFs that require pre-checking by validators in the active set.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn pvfs_require_precheck() -> Vec<v1::ValidationCodeHash>;

		/// Fetch the hash of the validation code used by a para, making the given `OccupiedCoreAssumption`.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn validation_code_hash(para_id: v1::Id, assumption: v1::OccupiedCoreAssumption)
			-> Option<v1::ValidationCodeHash>;
	}
}
