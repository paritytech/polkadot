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

use primitives::BlockNumber;
use sp_runtime::Perbill;
use frame_support::{
	parameter_types, traits::Currency,
	weights::{Weight, RuntimeDbWeight},
};

#[cfg(feature = "std")]
pub use staking::StakerStatus;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use timestamp::Call as TimestampCall;
pub use balances::Call as BalancesCall;
pub use attestations::{Call as AttestationsCall, MORE_ATTESTATIONS_IDENTIFIER};
pub use parachains::Call as ParachainsCall;

/// Implementations of some helper traits passed into runtime modules as associated types.
pub use impls::{CurrencyToVoteHandler, TargetedFeeAdjustment, ToAuthor};

pub type NegativeImbalance<T> = <balances::Module<T> as Currency<<T as system::Trait>::AccountId>>::NegativeImbalance;

parameter_types! {
	pub const BlockHashCount: BlockNumber = 250;
	pub const MaximumBlockWeight: Weight = 2_000_000_000_000;
	pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	pub const MaximumBlockLength: u32 = 5 * 1024 * 1024;
	/// Executing 10,000 System remarks (no-op) txs takes ~1.26 seconds -> ~125 µs per tx
	pub const ExtrinsicBaseWeight: Weight = 125_000_000;
	/// Importing a block with 0 txs takes ~5 ms
	pub const BlockExecutionWeight: Weight = 5_000_000_000;
	pub const DbWeight: RuntimeDbWeight = RuntimeDbWeight {
		read: 25_000_000, // ~25 µs @ 200,000 items
		write: 100_000_000, // ~100 µs @ 200,000 items
	};
}
