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

//! Types that we don't fetch from a particular runtime and just assume that they are constant all
//! of the place.
//!
//! It is actually easy to convert the rest as well, but it'll be a lot of noise in our codebase,
//! needing to sprinkle `any_runtime` in a few extra places.

use frame_support::{parameter_types, weights::Weight};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

/// The account id type.
pub type AccountId = subxt::sp_core::crypto::AccountId32;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header = subxt::sp_runtime::generic::Header<u32, subxt::sp_runtime::traits::BlakeTwo256>;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Hash = subxt::sp_core::H256;

pub use subxt::{
	sp_core,
	sp_runtime::traits::{Block as BlockT, Header as HeaderT},
};

/// Default URI to connect to.
pub const DEFAULT_URI: &str = "wss://rpc.polkadot.io:443";
/// The logging target.
pub const LOG_TARGET: &str = "staking-miner";

/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = subxt::sp_core::sr25519::Pair;

/// Type alias for the subxt client.
pub type SubxtClient = subxt::Client<subxt::DefaultConfig>;

pub use pallet_election_provider_multi_phase::{Miner, MinerConfig};

pub type Signer = subxt::PairSigner<subxt::DefaultConfig, subxt::sp_core::sr25519::Pair>;

pub use sp_runtime::Perbill;

pub type BoundedVec = frame_support::BoundedVec<AccountId, MinerMaxVotesPerVoter>;

parameter_types! {
	// TODO: this is part of the metadata check again if we can fetch this from subxt.
	pub BlockWeights: frame_system::limits::BlockWeights = frame_system::limits::BlockWeights
		::with_sensible_defaults(2 * frame_support::weights::constants::WEIGHT_PER_SECOND, NORMAL_DISPATCH_RATIO);
	pub static MinerMaxWeight: Weight = BlockWeights::get().max_block;
	// TODO: align with the chains
	pub static MinerMaxLength: u32 = 256;
	// TODO: align with the chains
	pub static MinerMaxVotesPerVoter: u32 = 256;
}

pub use crate::error::Error;
