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

use frame_support::{traits::ConstU32, weights::Weight};

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

pub use pallet_election_provider_multi_phase::{Miner, MinerConfig};

pub type Signer = subxt::PairSigner<subxt::DefaultConfig, subxt::sp_core::sr25519::Pair>;

pub use sp_runtime::Perbill;

pub type BoundedVec = frame_support::BoundedVec<AccountId, MaxVotesPerVoter>;

frame_support::parameter_types! {
	// TODO: this is part of the metadata check again if we can fetch this from subxt.
	// TODO: check with Kian.
	pub BlockWeights: frame_system::limits::BlockWeights = frame_system::limits::BlockWeights
		::with_sensible_defaults(u64::MAX, Perbill::from_percent(75));
	pub static MaxWeight: Weight = BlockWeights::get().max_block;
}

// TODO check with Kian.
pub type MaxLength = ConstU32<4294967295>;
// TODO check with Kian.
pub type MaxVotesPerVoter = ConstU32<256>;

pub use crate::error::Error;
