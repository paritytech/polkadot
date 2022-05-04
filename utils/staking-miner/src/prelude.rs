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

use codec::{Encode, Decode};
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

#[derive(codec::Encode, codec::Decode)]
pub struct SubmitCall<T>(Box<pallet_election_provider_multi_phase::RawSolution<T>>);

impl<T> SubmitCall<T> {
	pub fn new(raw_solution: pallet_election_provider_multi_phase::RawSolution<T>) -> Self {
		Self(Box::new(raw_solution))
	}
}

impl<T: Encode> subxt::Call for SubmitCall<T> {
	const PALLET: &'static str = "ElectionProviderMultiPhase";
	const FUNCTION: &'static str = "submit";
}

#[derive(Encode, Decode)]
pub struct ModuleErrMissing;

impl subxt::HasModuleError for ModuleErrMissing {
	fn module_error_data(&self) -> Option<subxt::ModuleErrorData> {
		None
	}
}

pub type NoEvents = String;

/// The account id type.
pub type AccountId = subxt::sp_core::crypto::AccountId32;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header = subxt::sp_runtime::generic::Header<u32, subxt::sp_runtime::traits::BlakeTwo256>;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Hash = subxt::sp_core::H256;

pub use subxt::sp_runtime::traits::{Block as BlockT, Header as HeaderT};

/// Default URI to connect to.
pub const DEFAULT_URI: &str = "wss://rpc.polkadot.io:443";
/// The logging target.
pub const LOG_TARGET: &str = "staking-miner";

/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = subxt::sp_core::sr25519::Pair;

/// Type alias for the subxt client.
pub type SubxtClient = subxt::Client<subxt::DefaultConfig>;

/// Runtime API.
pub type RuntimeApi = crate::runtime::RuntimeApi<
	subxt::DefaultConfig,
	subxt::SubstrateExtrinsicParams<subxt::DefaultConfig>,
>;

pub type ExtrinsicParams = subxt::SubstrateExtrinsicParams<subxt::DefaultConfig>;

pub use crate::runtime::runtime_types as runtime;

pub type Signer = subxt::PairSigner<subxt::DefaultConfig, subxt::sp_core::sr25519::Pair>;

pub mod polkadot {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MinerMaxLength;
		type MaxWeight = MinerMaxWeight;
		type MaxVotesPerVoter = MinerMaxVotesPerVotes;
		type Solution = polkadot::NposSolution16;

		fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
			// feasibility weight.
			(31_722_000 as Weight)
				// Standard Error: 8_000
				.saturating_add((1_255_000 as Weight).saturating_mul(v as Weight))
				// Standard Error: 28_000
				.saturating_add((8_972_000 as Weight).saturating_mul(a as Weight))
				// Standard Error: 42_000
				.saturating_add((966_000 as Weight).saturating_mul(d as Weight))
			//.saturating_add(T::DbWeight::get().reads(4 as Weight))
		}
	}
}

pub mod kusama {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MinerMaxLength;
		type MaxWeight = MinerMaxWeight;
		type MaxVotesPerVoter = MinerMaxVotesPerVotes;
		type Solution = NposSolution24;

		fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
			// feasibility weight.
			(31_722_000 as Weight)
				// Standard Error: 8_000
				.saturating_add((1_255_000 as Weight).saturating_mul(v as Weight))
				// Standard Error: 28_000
				.saturating_add((8_972_000 as Weight).saturating_mul(a as Weight))
				// Standard Error: 42_000
				.saturating_add((966_000 as Weight).saturating_mul(d as Weight))
			//.saturating_add(T::DbWeight::get().reads(4 as Weight))
		}
	}
}

/// Staking miner for Polkadot.
pub type PolkadotMiner = pallet_election_provider_multi_phase::unsigned::Miner<polkadot::Config>;

/// Staking miner for Kusama,
pub type KusamaMiner = pallet_election_provider_multi_phase::unsigned::Miner<kusama::Config>;
