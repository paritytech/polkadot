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

//! Code for elections.

use super::{BlockExecutionWeight, BlockLength, BlockWeights};
use frame_support::{
	parameter_types,
	weights::{DispatchClass, Weight},
};
use sp_runtime::Perbill;

parameter_types! {
	/// A limit for off-chain phragmen unsigned solution submission.
	///
	/// We want to keep it as high as possible, but can't risk having it reject,
	/// so we always subtract the base block execution weight.
	pub OffchainSolutionWeightLimit: Weight = BlockWeights::get()
		.get(DispatchClass::Normal)
		.max_extrinsic
		.expect("Normal extrinsics have weight limit configured by default; qed")
		.saturating_sub(BlockExecutionWeight::get());

	/// A limit for off-chain phragmen unsigned solution length.
	///
	/// We allow up to 90% of the block's size to be consumed by the solution.
	pub OffchainSolutionLengthLimit: u32 = Perbill::from_rational(90_u32, 100) *
		*BlockLength::get()
		.max
		.get(DispatchClass::Normal);
}

/// The numbers configured here should always be more than the the maximum limits of staking pallet
/// to ensure election snapshot will not run out of memory.
pub struct BenchmarkConfig;
impl pallet_election_provider_multi_phase::BenchmarkingConfig for BenchmarkConfig {
	const VOTERS: [u32; 2] = [5_000, 10_000];
	const TARGETS: [u32; 2] = [1_000, 2_000];
	const ACTIVE_VOTERS: [u32; 2] = [1000, 4_000];
	const DESIRED_TARGETS: [u32; 2] = [400, 800];
	const SNAPSHOT_MAXIMUM_VOTERS: u32 = 25_000;
	const MINER_MAXIMUM_VOTERS: u32 = 15_000;
	const MAXIMUM_TARGETS: u32 = 2000;
}

/// The accuracy type used for genesis election provider;
pub type OnOnChainAccuracy = sp_runtime::Perbill;

/// The election provider of the genesis
pub type GenesisElectionOf<T> =
	frame_election_provider_support::onchain::OnChainSequentialPhragmen<T>;

/// Maximum number of iterations for balancing that will be executed in the embedded miner of
/// pallet-election-provider-multi-phase.
pub const MINER_MAX_ITERATIONS: u32 = 10;

/// A source of random balance for the NPoS Solver, which is meant to be run by the offchain worker
/// election miner.
pub struct OffchainRandomBalancing;
impl frame_support::pallet_prelude::Get<Option<(usize, sp_npos_elections::ExtendedBalance)>>
	for OffchainRandomBalancing
{
	fn get() -> Option<(usize, sp_npos_elections::ExtendedBalance)> {
		use sp_runtime::{codec::Decode, traits::TrailingZeroInput};
		let iters = match MINER_MAX_ITERATIONS {
			0 => 0,
			max @ _ => {
				let seed = sp_io::offchain::random_seed();
				let random = <u32>::decode(&mut TrailingZeroInput::new(&seed))
					.expect("input is padded with zeroes; qed") %
					max.saturating_add(1);
				random as usize
			},
		};

		Some((iters, 0))
	}
}
