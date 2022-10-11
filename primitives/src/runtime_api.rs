// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

//! Runtime API module declares the `trait ParachainHost` which is part
//! of the Runtime API exposed from the Runtime to the Host.
//!
//! The functions in trait ParachainHost` can be part of the stable API
//! (which is versioned) or they can be staging (aka unstable/testing
//! functions).
//!
//! The separation outlined above is achieved with the versioned API feature
//! of `decl_runtime_apis!` and `impl_runtime_apis!`. Before moving on let's
//! see a quick example about how API versioning works.
//!
//! # Runtime API versioning crash course
//!
//! The versioning is achieved with the `api_version` attribute. It can be
//! placed on:
//! * trait declaration - represents the base version of the API.
//! * method declaration (inside a trait declaration) - represents a versioned
//!   method, which is not available in the base version.
//! * trait implementation - represents which version of the API is being
//!   implemented.
//!
//! Let's see a quick example:
//!
//! ```rust(ignore)
//! sp_api::decl_runtime_apis! {
//! 	#[api_version(2)]
//! 	pub trait MyApi {
//! 		fn fn1();
//! 		fn fn2();
//! 		#[api_version(3)]
//! 		fn fn3();
//! 		#[api_version(4)]
//! 		fn fn4();
//! 	}
//! }
//!
//! struct Runtime {}
//!
//! sp_api::impl_runtime_apis! {
//!     #[api_version(3)]
//!     impl self::MyApi<Block> for Runtime {
//!         fn fn1() {}
//!         fn fn2() {}
//!         fn fn3() {}
//!     }
//! }
//! ```
//! A new API named `MyApi` is declared with `decl_runtime_apis!`. The trait declaration
//! has got an `api_version` attribute which represents its base version - 2 in this case.
//!
//! The API has got three methods - `fn1`, `fn2`, `fn3` and `fn4`. `fn3` and `fn4` has got
//! an `api_version` attribute which makes them versioned methods. These methods do not exist
//! in the base version of the API. Behind the scenes the declaration above creates three
//! runtime APIs:
//! * `MyApiV2` with `fn1` and `fn2`
//! * `MyApiV3` with `fn1`, `fn2` and `fn3`.
//! * `MyApiV4` with `fn1`, `fn2`, `fn3` and `fn4`.
//!
//! Please note that `v4` contains all methods from `v3`, `v3` all methods from `v2` and so on.
//!
//! Back to our example. At the end runtime API is implemented for `struct Runtime` with
//! `impl_runtime_apis` macro. `api_version` attribute is attached to the `impl` block which
//! means that a version different from the base one is being implemented - in our case this
//! is `v3`.
//!
//! This version of the API contains three methods so the `impl` block has got definitions
//! for them. Note that `fn4` is not implemented as it is not part of this version of the API.
//! `impl_runtime_apis` generates a default implementation for it calling `unimplemented!()`.
//!
//! Hopefully this should be all you need to know in order to use versioned methods in the node.
//! For more details about how the API versioning works refer to `spi_api`
//! documentation [here](https://docs.substrate.io/rustdocs/latest/sp_api/macro.decl_runtime_apis.html).
//!
//! # How versioned methods are used for `ParachainHost`
//!
//! Let's introduce two types of `ParachainHost` API implementation:
//! * stable - used on stable production networks like Polkadot and Kusama. There is only one
//!   stable API at a single point in time.
//! * staging - used on test networks like Westend or Rococo. Depending on the development needs
//!   there can be zero, one or multiple staging APIs.
//!
//! The stable version of `ParachainHost` is indicated by the base version of the API. Any staging
//! method must use `api_version` attribute so that it is assigned to a specific version of a
//! staging API. This way in a single declaration one can see what's the stable version of
//! `ParachainHost` and what staging versions/functions are available.
//!
//! All stable API functions should use primitives from the latest version.
//! In the time of writing of this document - this is `v2`. So for example:
//! ```ignore
//! fn validators() -> Vec<v2::ValidatorId>;
//! ```
//! indicates a function from the stable `v2` API.
//!
//! All staging API functions should use primitives from `vstaging`. They should be clearly separated
//! from the stable primitives.

use crate::v2;
use parity_scale_codec::{Decode, Encode};
use polkadot_core_primitives as pcp;
use polkadot_parachain::primitives as ppp;
use sp_staking;
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

sp_api::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	#[api_version(2)]
	pub trait ParachainHost<H: Encode + Decode = pcp::v2::Hash, N: Encode + Decode = pcp::v2::BlockNumber> {
		/// Get the current validators.
		fn validators() -> Vec<v2::ValidatorId>;

		/// Returns the validator groups and rotation info localized based on the hypothetical child
		///  of a block whose state  this is invoked on. Note that `now` in the `GroupRotationInfo`
		/// should be the successor of the number of the block.
		fn validator_groups() -> (Vec<Vec<v2::ValidatorIndex>>, v2::GroupRotationInfo<N>);

		/// Yields information on all availability cores as relevant to the child block.
		/// Cores are either free or occupied. Free cores can have paras assigned to them.
		fn availability_cores() -> Vec<v2::CoreState<H, N>>;

		/// Yields the persisted validation data for the given `ParaId` along with an assumption that
		/// should be used if the para currently occupies a core.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		fn persisted_validation_data(para_id: ppp::Id, assumption: v2::OccupiedCoreAssumption)
			-> Option<v2::PersistedValidationData<H, N>>;

		/// Returns the persisted validation data for the given `ParaId` along with the corresponding
		/// validation code hash. Instead of accepting assumption about the para, matches the validation
		/// data hash against an expected one and yields `None` if they're not equal.
		fn assumed_validation_data(
			para_id: ppp::Id,
			expected_persisted_validation_data_hash: pcp::v2::Hash,
		) -> Option<(v2::PersistedValidationData<H, N>, ppp::ValidationCodeHash)>;

		/// Checks if the given validation outputs pass the acceptance criteria.
		fn check_validation_outputs(para_id: ppp::Id, outputs: v2::CandidateCommitments) -> bool;

		/// Returns the session index expected at a child of the block.
		///
		/// This can be used to instantiate a `SigningContext`.
		fn session_index_for_child() -> sp_staking::SessionIndex;

		/// Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		fn validation_code(para_id: ppp::Id, assumption: v2::OccupiedCoreAssumption)
			-> Option<ppp::ValidationCode>;

		/// Get the receipt of a candidate pending availability. This returns `Some` for any paras
		/// assigned to occupied cores in `availability_cores` and `None` otherwise.
		fn candidate_pending_availability(para_id: ppp::Id) -> Option<v2::CommittedCandidateReceipt<H>>;

		/// Get a vector of events concerning candidates that occurred within a block.
		fn candidate_events() -> Vec<v2::CandidateEvent<H>>;

		/// Get all the pending inbound messages in the downward message queue for a para.
		fn dmq_contents(
			recipient: ppp::Id,
		) -> Vec<pcp::v2::InboundDownwardMessage<N>>;

		/// Get the contents of all channels addressed to the given recipient. Channels that have no
		/// messages in them are also included.
		fn inbound_hrmp_channels_contents(recipient: ppp::Id) -> BTreeMap<ppp::Id, Vec<pcp::v2::InboundHrmpMessage<N>>>;

		/// Get the validation code from its hash.
		fn validation_code_by_hash(hash: ppp::ValidationCodeHash) -> Option<ppp::ValidationCode>;

		/// Scrape dispute relevant from on-chain, backing votes and resolved disputes.
		fn on_chain_votes() -> Option<v2::ScrapedOnChainVotes<H>>;

		/***** Added in v2 *****/

		/// Get the session info for the given session, if stored.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn session_info(index: sp_staking::SessionIndex) -> Option<v2::SessionInfo>;

		/// Submits a PVF pre-checking statement into the transaction pool.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn submit_pvf_check_statement(stmt: v2::PvfCheckStatement, signature: v2::ValidatorSignature);

		/// Returns code hashes of PVFs that require pre-checking by validators in the active set.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn pvfs_require_precheck() -> Vec<ppp::ValidationCodeHash>;

		/// Fetch the hash of the validation code used by a para, making the given `OccupiedCoreAssumption`.
		///
		/// NOTE: This function is only available since parachain host version 2.
		fn validation_code_hash(para_id: ppp::Id, assumption: v2::OccupiedCoreAssumption)
			-> Option<ppp::ValidationCodeHash>;


		/***** Replaced in v2 *****/

		/// Old method to fetch v1 session info.
		#[changed_in(2)]
		fn session_info(index: sp_staking::SessionIndex) -> Option<v2::OldV1SessionInfo>;

		/***** STAGING *****/

		/// Returns all onchain disputes.
		#[api_version(3)]
		fn disputes() -> Vec<(v2::SessionIndex, v2::CandidateHash, v2::DisputeState<v2::BlockNumber>)>;
	}
}
