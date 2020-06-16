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
pub mod impls;

use sp_std::marker::PhantomData;
use codec::{Encode, Decode};
use primitives::{BlockNumber, AccountId, ValidityError};
use sp_runtime::{
	Perquintill, Perbill, FixedPointNumber,
	traits::{Saturating, SignedExtension, DispatchInfoOf},
	transaction_validity::{TransactionValidityError, TransactionValidity, InvalidTransaction}
};
use frame_support::{
	parameter_types, traits::{Currency, Filter},
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
use sp_runtime::traits::Dispatchable;

pub type NegativeImbalance<T> = <balances::Module<T> as Currency<<T as system::Trait>::AccountId>>::NegativeImbalance;

const AVERAGE_ON_INITIALIZE_WEIGHT: Perbill = Perbill::from_percent(10);
parameter_types! {
	pub const BlockHashCount: BlockNumber = 2400;
	pub const MaximumBlockWeight: Weight = 2 * WEIGHT_PER_SECOND;
	pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	pub MaximumExtrinsicWeight: Weight = AvailableBlockRatio::get()
		.saturating_sub(AVERAGE_ON_INITIALIZE_WEIGHT) * MaximumBlockWeight::get();
	pub const MaximumBlockLength: u32 = 5 * 1024 * 1024;
}

const_assert!(AvailableBlockRatio::get().deconstruct() >= AVERAGE_ON_INITIALIZE_WEIGHT.deconstruct());

/// Apply a given filter to transactions.
pub struct TransactionCallFilter<T: Filter<Call>, Call>(PhantomData<(T, Call)>);

impl<F: Filter<Call>, Call> Default for TransactionCallFilter<F, Call> {
	fn default() -> Self { Self::new() }
}
impl<F: Filter<Call>, Call> Encode for TransactionCallFilter<F, Call> {
	fn using_encoded<R, FO: FnOnce(&[u8]) -> R>(&self, f: FO) -> R { f(&b""[..]) }
}
impl<F: Filter<Call>, Call> Decode for TransactionCallFilter<F, Call> {
	fn decode<I: codec::Input>(_: &mut I) -> Result<Self, codec::Error> { Ok(Self::new()) }
}
impl<F: Filter<Call>, Call> Clone for TransactionCallFilter<F, Call> {
	fn clone(&self) -> Self { Self::new() }
}
impl<F: Filter<Call>, Call> Eq for TransactionCallFilter<F, Call> {}
impl<F: Filter<Call>, Call> PartialEq for TransactionCallFilter<F, Call> {
	fn eq(&self, _: &Self) -> bool { true }
}
impl<F: Filter<Call>, Call> sp_std::fmt::Debug for TransactionCallFilter<F, Call> {
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result { Ok(()) }
}

fn validate<F: Filter<Call>, Call>(call: &Call) -> TransactionValidity {
	if F::filter(call) {
		Ok(Default::default())
	} else {
		Err(InvalidTransaction::Custom(ValidityError::NoPermission.into()).into())
	}
}

impl<F: Filter<Call> + Send + Sync, Call: Dispatchable + Send + Sync>
	SignedExtension for TransactionCallFilter<F, Call>
{
	const IDENTIFIER: &'static str = "TransactionCallFilter";
	type AccountId = AccountId;
	type Call = Call;
	type AdditionalSigned = ();
	type Pre = ();

	fn additional_signed(&self) -> sp_std::result::Result<(), TransactionValidityError> { Ok(()) }

	fn validate(&self,
		_: &Self::AccountId,
		call: &Call,
		_: &DispatchInfoOf<Self::Call>,
		_: usize,
	) -> TransactionValidity { validate::<F, _>(call) }

	fn validate_unsigned(
		call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		_len: usize,
	) -> TransactionValidity { validate::<F, _>(call) }
}

impl<F: Filter<Call>, Call> TransactionCallFilter<F, Call> {
	/// Create a new instance.
	pub fn new() -> Self {
		Self(sp_std::marker::PhantomData)
	}
}

parameter_types! {
	pub const TargetBlockFullness: Perquintill = Perquintill::from_percent(25);
	pub AdjustmentVariable: Multiplier = Multiplier::saturating_from_rational(3, 100_000);
	pub MinimumMultiplier: Multiplier = Multiplier::saturating_from_rational(1, 1_000_000_000u128);
}

/// Slow adjusting fee updated based on
/// https://w3f-research.readthedocs.io/en/latest/polkadot/Token%20Economics.html#-2.-slow-adjusting-mechanism
pub type SlowAdjustingFeeUpdate<R> = TargetedFeeAdjustment<R, TargetBlockFullness, AdjustmentVariable, MinimumMultiplier>;

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
