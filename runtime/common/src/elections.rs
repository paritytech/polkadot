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

use frame_election_provider_support::{
	onchain::{ExecutionConfig, UnboundedExecution},
	ElectionDataProvider, SequentialPhragmen,
};
use sp_std::marker::PhantomData;

/// Implements the weight types for the elections module and a specific
/// runtime.
/// This macro should not be called directly; use [`impl_runtime_weights`] instead.
#[macro_export]
macro_rules! impl_elections_weights {
	($runtime:ident) => {
		parameter_types! {
			/// A limit for off-chain phragmen unsigned solution submission.
			///
			/// We want to keep it as high as possible, but can't risk having it reject,
			/// so we always subtract the base block execution weight.
			pub OffchainSolutionWeightLimit: Weight = BlockWeights::get()
				.get(DispatchClass::Normal)
				.max_extrinsic
				.expect("Normal extrinsics have weight limit configured by default; qed")
				.saturating_sub($runtime::weights::BlockExecutionWeight::get());

			/// A limit for off-chain phragmen unsigned solution length.
			///
			/// We allow up to 90% of the block's size to be consumed by the solution.
			pub OffchainSolutionLengthLimit: u32 = Perbill::from_rational(90_u32, 100) *
				*BlockLength::get()
				.max
				.get(DispatchClass::Normal);
		}
	};
}

/// The numbers configured here could always be more than the the maximum limits of staking pallet
/// to ensure election snapshot will not run out of memory. For now, we set them to smaller values
/// since the staking is bounded and the weight pipeline takes hours for this single pallet.
pub struct BenchmarkConfig;
impl pallet_election_provider_multi_phase::BenchmarkingConfig for BenchmarkConfig {
	const VOTERS: [u32; 2] = [1000, 2000];
	const TARGETS: [u32; 2] = [500, 1000];
	const ACTIVE_VOTERS: [u32; 2] = [500, 800];
	const DESIRED_TARGETS: [u32; 2] = [200, 400];
	const SNAPSHOT_MAXIMUM_VOTERS: u32 = 1000;
	const MINER_MAXIMUM_VOTERS: u32 = 1000;
	const MAXIMUM_TARGETS: u32 = 300;
}

/// The accuracy type used for genesis election provider;
pub type OnOnChainAccuracy = sp_runtime::Perbill;

/// Election Configuration parameters
pub struct OnChainSeqPhragmen<T: frame_system::Config, S>(PhantomData<(T, S)>);
impl<
		T: frame_system::Config,
		S: ElectionDataProvider<AccountId = T::AccountId, BlockNumber = T::BlockNumber>,
	> ExecutionConfig for OnChainSeqPhragmen<T, S>
{
	type System = T;
	type Solver = SequentialPhragmen<T::AccountId, OnOnChainAccuracy>;
	type DataProvider = S;
}

/// The election provider of the genesis
pub type GenesisElectionOf<T, S> = UnboundedExecution<OnChainSeqPhragmen<T, S>>;
