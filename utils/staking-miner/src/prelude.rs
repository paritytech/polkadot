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

use frame_support::{parameter_types, traits::ConstU32, weights::Weight};
use sp_runtime::{PerU16, Perbill};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

parameter_types! {
	pub BlockWeights: frame_system::limits::BlockWeights = frame_system::limits::BlockWeights
		::with_sensible_defaults(2 * frame_support::weights::constants::WEIGHT_PER_SECOND, NORMAL_DISPATCH_RATIO);
	pub static MinerMaxWeight: Weight = BlockWeights::get().max_block;
	pub static MinerMaxLength: u32 = 256;
	pub static MinerMaxVotesPerVotes: u32 = 16;
}

frame_election_provider_support::generate_solution_type!(
	#[compact]
	pub struct MockedNposSolution::<
		VoterIndex = u32,
		TargetIndex = u16,
		Accuracy = PerU16,
		MaxVoters = ConstU32::<2_000>
	>(16)
);

/// The account id type.
pub type AccountId = subxt::sp_core::crypto::AccountId32;
/// The block number type.
pub type BlockNumber = core_primitives::BlockNumber;
/// The balance type.
pub type Balance = core_primitives::Balance;
/// The index of an account.
pub type Index = core_primitives::AccountIndex;
/// The hash type. We re-export it here, but we can easily get it from block as well.
pub type Hash = core_primitives::Hash;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header = core_primitives::Header;
/// The block type.
pub type Block = core_primitives::Block;

pub use subxt::sp_runtime::traits::{Block as BlockT, Header as HeaderT};

/// Default URI to connect to.
pub const DEFAULT_URI: &str = "wss://rpc.polkadot.io:443";
/// The logging target.
pub const LOG_TARGET: &str = "staking-miner";

/// The externalities type.
pub type Ext = sp_io::TestExternalities;

/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = subxt::sp_core::sr25519::Pair;

/// Type alias for the subxt client.
pub type SubxtClient = subxt::Client<subxt::DefaultConfig>;

/// Runtime API.
pub type RuntimeApi = crate::runtime::RuntimeApi<
	subxt::DefaultConfig,
	subxt::PolkadotExtrinsicParams<subxt::DefaultConfig>,
>;

pub type ExtrinsicParams = subxt::PolkadotExtrinsicParams<subxt::DefaultConfig>;

pub use crate::runtime::runtime_types as runtime;

pub type Signer = subxt::PairSigner<subxt::DefaultConfig, subxt::sp_core::sr25519::Pair>;

pub struct MockedMiner;

impl pallet_election_provider_multi_phase::unsigned::MinerConfig for MockedMiner {
	type AccountId = AccountId;
	type MaxLength = MinerMaxLength;
	type MaxWeight = MinerMaxWeight;
	type MaxVotesPerVoter = MinerMaxVotesPerVotes;
	type Solution = MockedNposSolution;

	fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
		todo!();
		// match MockWeightInfo::get() {
		//     MockedWeightInfo::Basic =>
		//         (10 as Weight).saturating_add((5 as Weight).saturating_mul(a as Weight)),
		//     MockedWeightInfo::Complex => (0 * v + 0 * t + 1000 * a + 0 * d) as Weight,
		//     MockedWeightInfo::Real =>
		//         <() as multi_phase::weights::WeightInfo>::feasibility_check(v, t, a, d),
		// }
	}
}

pub type Miner = pallet_election_provider_multi_phase::unsigned::Miner<MockedMiner>;
