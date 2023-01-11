// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

pub mod assigned_slots;
pub mod auctions;
pub mod claims;
pub mod crowdloan;
pub mod elections;
pub mod impls;
pub mod paras_registrar;
pub mod paras_sudo_wrapper;
pub mod purchase;
pub mod slot_range;
pub mod slots;
pub mod traits;
pub mod xcm_sender;

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod mock;

use frame_support::{
	parameter_types,
	traits::{ConstU32, Currency, OneSessionHandler},
	weights::{constants::WEIGHT_PER_SECOND, Weight},
};
use frame_system::limits;
use primitives::v2::{AssignmentId, Balance, BlockNumber, ValidatorId, MAX_POV_SIZE};
use sp_runtime::{FixedPointNumber, Perbill, Perquintill};
use static_assertions::const_assert;

pub use pallet_balances::Call as BalancesCall;
#[cfg(feature = "std")]
pub use pallet_staking::StakerStatus;
pub use pallet_timestamp::Call as TimestampCall;
use pallet_transaction_payment::{Multiplier, TargetedFeeAdjustment};
pub use sp_runtime::traits::Bounded;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

/// Implementations of some helper traits passed into runtime modules as associated types.
pub use impls::ToAuthor;

pub type NegativeImbalance<T> = <pallet_balances::Pallet<T> as Currency<
	<T as frame_system::Config>::AccountId,
>>::NegativeImbalance;

/// We assume that an on-initialize consumes 1% of the weight on average, hence a single extrinsic
/// will not be allowed to consume more than `AvailableBlockRatio - 1%`.
pub const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(1);
/// We allow `Normal` extrinsics to fill up the block up to 75%, the rest can be used
/// by  Operational  extrinsics.
pub const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
/// The storage proof size is not limited so far.
pub const MAXIMUM_BLOCK_WEIGHT: Weight =
	WEIGHT_PER_SECOND.saturating_mul(2).set_proof_size(MAX_POV_SIZE as u64);

const_assert!(NORMAL_DISPATCH_RATIO.deconstruct() >= AVERAGE_ON_INITIALIZE_RATIO.deconstruct());

// Common constants used in all runtimes.
parameter_types! {
	pub const BlockHashCount: BlockNumber = 4096;
	/// The portion of the `NORMAL_DISPATCH_RATIO` that we adjust the fees with. Blocks filled less
	/// than this will decrease the weight and more will increase.
	pub const TargetBlockFullness: Perquintill = Perquintill::from_percent(25);
	/// The adjustment variable of the runtime. Higher values will cause `TargetBlockFullness` to
	/// change the fees more rapidly.
	pub AdjustmentVariable: Multiplier = Multiplier::saturating_from_rational(75, 1000_000);
	/// Minimum amount of the multiplier. This value cannot be too low. A test case should ensure
	/// that combined with `AdjustmentVariable`, we can recover from the minimum.
	/// See `multiplier_can_grow_from_zero`.
	pub MinimumMultiplier: Multiplier = Multiplier::saturating_from_rational(1, 10u128);
	/// The maximum amount of the multiplier.
	pub MaximumMultiplier: Multiplier = Bounded::max_value();
	/// Maximum length of block. Up to 5MB.
	pub BlockLength: limits::BlockLength =
	limits::BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
}

/// Parameterized slow adjusting fee updated based on
/// https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html#-2.-slow-adjusting-mechanism
pub type SlowAdjustingFeeUpdate<R> = TargetedFeeAdjustment<
	R,
	TargetBlockFullness,
	AdjustmentVariable,
	MinimumMultiplier,
	MaximumMultiplier,
>;

/// Implements the weight types for a runtime.
/// It expects the passed runtime constants to contain a `weights` module.
/// The generated weight types were formerly part of the common
/// runtime but are now runtime dependant.
#[macro_export]
macro_rules! impl_runtime_weights {
	($runtime:ident) => {
		use frame_support::{dispatch::DispatchClass, weights::Weight};
		use frame_system::limits;
		use pallet_transaction_payment::{Multiplier, TargetedFeeAdjustment};
		pub use runtime_common::{
			impl_elections_weights, AVERAGE_ON_INITIALIZE_RATIO, MAXIMUM_BLOCK_WEIGHT,
			NORMAL_DISPATCH_RATIO,
		};
		use sp_runtime::{FixedPointNumber, Perquintill};

		// Implement the weight types of the elections module.
		impl_elections_weights!($runtime);

		// Expose the weight from the runtime constants module.
		pub use $runtime::weights::{
			BlockExecutionWeight, ExtrinsicBaseWeight, ParityDbWeight, RocksDbWeight,
		};

		parameter_types! {
			/// Block weights base values and limits.
			pub BlockWeights: limits::BlockWeights = limits::BlockWeights::builder()
				.base_block($runtime::weights::BlockExecutionWeight::get())
				.for_class(DispatchClass::all(), |weights| {
					weights.base_extrinsic = $runtime::weights::ExtrinsicBaseWeight::get();
				})
				.for_class(DispatchClass::Normal, |weights| {
					weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
				})
				.for_class(DispatchClass::Operational, |weights| {
					weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
					// Operational transactions have an extra reserved space, so that they
					// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
					weights.reserved = Some(
						MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT,
					);
				})
				.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
				.build_or_panic();
		}
	};
}

/// The type used for currency conversion.
///
/// This must only be used as long as the balance type is `u128`.
pub type CurrencyToVote = frame_support::traits::U128CurrencyToVote;
static_assertions::assert_eq_size!(primitives::v2::Balance, u128);

/// A placeholder since there is currently no provided session key handler for parachain validator
/// keys.
pub struct ParachainSessionKeyPlaceholder<T>(sp_std::marker::PhantomData<T>);
impl<T> sp_runtime::BoundToRuntimeAppPublic for ParachainSessionKeyPlaceholder<T> {
	type Public = ValidatorId;
}

impl<T: pallet_session::Config> OneSessionHandler<T::AccountId>
	for ParachainSessionKeyPlaceholder<T>
{
	type Key = ValidatorId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
	where
		I: Iterator<Item = (&'a T::AccountId, ValidatorId)>,
		T::AccountId: 'a,
	{
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, _v: I, _q: I)
	where
		I: Iterator<Item = (&'a T::AccountId, ValidatorId)>,
		T::AccountId: 'a,
	{
	}

	fn on_disabled(_: u32) {}
}

/// A placeholder since there is currently no provided session key handler for parachain validator
/// keys.
pub struct AssignmentSessionKeyPlaceholder<T>(sp_std::marker::PhantomData<T>);
impl<T> sp_runtime::BoundToRuntimeAppPublic for AssignmentSessionKeyPlaceholder<T> {
	type Public = AssignmentId;
}

impl<T: pallet_session::Config> OneSessionHandler<T::AccountId>
	for AssignmentSessionKeyPlaceholder<T>
{
	type Key = AssignmentId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
	where
		I: Iterator<Item = (&'a T::AccountId, AssignmentId)>,
		T::AccountId: 'a,
	{
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, _v: I, _q: I)
	where
		I: Iterator<Item = (&'a T::AccountId, AssignmentId)>,
		T::AccountId: 'a,
	{
	}

	fn on_disabled(_: u32) {}
}

/// A reasonable benchmarking config for staking pallet.
pub struct StakingBenchmarkingConfig;
impl pallet_staking::BenchmarkingConfig for StakingBenchmarkingConfig {
	type MaxValidators = ConstU32<1000>;
	type MaxNominators = ConstU32<1000>;
}

/// Convert a balance to an unsigned 256-bit number, use in nomination pools.
pub struct BalanceToU256;
impl sp_runtime::traits::Convert<Balance, sp_core::U256> for BalanceToU256 {
	fn convert(n: Balance) -> sp_core::U256 {
		n.into()
	}
}

/// Convert an unsigned 256-bit number to balance, use in nomination pools.
pub struct U256ToBalance;
impl sp_runtime::traits::Convert<sp_core::U256, Balance> for U256ToBalance {
	fn convert(n: sp_core::U256) -> Balance {
		use frame_support::traits::Defensive;
		n.try_into().defensive_unwrap_or(Balance::MAX)
	}
}

/// Macro to set a value (e.g. when using the `parameter_types` macro) to either a production value
/// or to an environment variable or testing value (in case the `fast-runtime` feature is selected).
/// Note that the environment variable is evaluated _at compile time_.
///
/// Usage:
/// ```Rust
/// parameter_types! {
/// 	// Note that the env variable version parameter cannot be const.
/// 	pub LaunchPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1, "KSM_LAUNCH_PERIOD");
/// 	pub const VotingPeriod: BlockNumber = prod_or_fast!(7 * DAYS, 1 * MINUTES);
/// }
/// ```
#[macro_export]
macro_rules! prod_or_fast {
	($prod:expr, $test:expr) => {
		if cfg!(feature = "fast-runtime") {
			$test
		} else {
			$prod
		}
	};
	($prod:expr, $test:expr, $env:expr) => {
		if cfg!(feature = "fast-runtime") {
			core::option_env!($env).map(|s| s.parse().ok()).flatten().unwrap_or($test)
		} else {
			$prod
		}
	};
}
