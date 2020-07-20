// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Common runtime code for Polkadot and Kusama.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod attestations;
pub mod claims;
pub mod parachains;
pub mod slot_range;
pub mod registrar;
pub mod slots;
pub mod crowdfund;
pub mod purchase;
pub mod impls;

use primitives::v0::BlockNumber;
use sp_runtime::{Perquintill, Perbill, FixedPointNumber, traits::Saturating};
use frame_support::{
	parameter_types, traits::{Currency},
	weights::{Weight, constants::WEIGHT_PER_SECOND},
};
use transaction_payment::{TargetedFeeAdjustment, Multiplier};
use static_assertions::const_assert;
pub use frame_support::weights::constants::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight};

#[cfg(feature = "std")]
pub use staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use timestamp::Call as TimestampCall;
pub use balances::Call as BalancesCall;
pub use attestations::{Call as AttestationsCall, MORE_ATTESTATIONS_IDENTIFIER};
pub use parachains::Call as ParachainsCall;

/// Implementations of some helper traits passed into runtime modules as associated types.
pub use impls::{CurrencyToVoteHandler, ToAuthor};

pub type NegativeImbalance<T> = <balances::Module<T> as Currency<<T as system::Trait>::AccountId>>::NegativeImbalance;

/// We assume that an on-initialize consumes 10% of the weight on average, hence a single extrinsic
/// will not be allowed to consume more than `AvailableBlockRatio - 10%`.
const AVERAGE_ON_INITIALIZE_WEIGHT: Perbill = Perbill::from_percent(10);

// Common constants used in all runtimes.
parameter_types! {
	pub const BlockHashCount: BlockNumber = 2400;
	/// Block time that can be used by weights.
	pub const MaximumBlockWeight: Weight = 2 * WEIGHT_PER_SECOND;
	/// Portion of the block available to normal class of dispatches.
	pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	/// Maximum weight that a _single_ extrinsic can take.
	pub MaximumExtrinsicWeight: Weight = AvailableBlockRatio::get()
		.saturating_sub(AVERAGE_ON_INITIALIZE_WEIGHT) * MaximumBlockWeight::get();
	/// Maximum length of block. 5MB.
	pub const MaximumBlockLength: u32 = 5 * 1024 * 1024;
	/// The portion of the `AvailableBlockRatio` that we adjust the fees with. Blocks filled less
	/// than this will decrease the weight and more will increase.
	pub const TargetBlockFullness: Perquintill = Perquintill::from_percent(25);
	/// The adjustment variable of the runtime. Higher values will cause `TargetBlockFullness` to
	/// change the fees more rapidly.
	pub AdjustmentVariable: Multiplier = Multiplier::saturating_from_rational(3, 100_000);
	/// Minimum amount of the multiplier. This value cannot be too low. A test case should ensure
	/// that combined with `AdjustmentVariable`, we can recover from the minimum.
	/// See `multiplier_can_grow_from_zero`.
	pub MinimumMultiplier: Multiplier = Multiplier::saturating_from_rational(1, 1_000_000_000u128);
}

const_assert!(AvailableBlockRatio::get().deconstruct() >= AVERAGE_ON_INITIALIZE_WEIGHT.deconstruct());

/// Parameterized slow adjusting fee updated based on
/// https://w3f-research.readthedocs.io/en/latest/polkadot/Token%20Economics.html#-2.-slow-adjusting-mechanism
pub type SlowAdjustingFeeUpdate<R> = TargetedFeeAdjustment<
	R,
	TargetBlockFullness,
	AdjustmentVariable,
	MinimumMultiplier
>;

#[cfg(test)]
mod multiplier_tests {
	use super::*;
	use frame_support::{impl_outer_origin, parameter_types, weights::Weight};
	use sp_core::H256;
	use sp_runtime::{
		testing::Header,
		traits::{BlakeTwo256, IdentityLookup, Convert},
		Perbill,
	};

	#[derive(Clone, PartialEq, Eq, Debug)]
	pub struct Runtime;

	impl_outer_origin!{
		pub enum Origin for Runtime {}
	}

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const ExtrinsicBaseWeight: u64 = 100;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}

	impl system::Trait for Runtime {
		type BaseCallFilter = ();
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Call = ();
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type DbWeight = ();
		type BlockExecutionWeight = ();
		type ExtrinsicBaseWeight = ExtrinsicBaseWeight;
		type MaximumExtrinsicWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
		type AccountData = ();
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
	}

	type System = system::Module<Runtime>;

	fn run_with_system_weight<F>(w: Weight, assertions: F) where F: Fn() -> () {
		let mut t: sp_io::TestExternalities =
			system::GenesisConfig::default().build_storage::<Runtime>().unwrap().into();
		t.execute_with(|| {
			System::set_block_limits(w, 0);
			assertions()
		});
	}

	#[test]
	fn multiplier_can_grow_from_zero() {
		let minimum_multiplier = MinimumMultiplier::get();
		let target = TargetBlockFullness::get() * (AvailableBlockRatio::get() * MaximumBlockWeight::get());
		// if the min is too small, then this will not change, and we are doomed forever.
		// the weight is 1/10th bigger than target.
		run_with_system_weight(target * 101 / 100, || {
			let next = SlowAdjustingFeeUpdate::<Runtime>::convert(minimum_multiplier);
			assert!(next > minimum_multiplier, "{:?} !>= {:?}", next, minimum_multiplier);
		})
	}
}
