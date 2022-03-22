// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

#![cfg_attr(not(feature = "std"), no_std)]
// RuntimeApi generated functions
#![allow(clippy::too_many_arguments)]

use bp_runtime::Chain;
use frame_support::{
	weights::{constants::WEIGHT_PER_SECOND, DispatchClass, IdentityFee, Weight},
	RuntimeDebug,
};
use frame_system::limits;
use sp_core::Hasher as HasherT;
use sp_runtime::{
	traits::{BlakeTwo256, IdentifyAccount, Verify},
	MultiSignature, MultiSigner, Perbill,
};

/// Maximal weight of single Rialto parachain block.
///
/// This represents two seconds of compute assuming a target block time of six seconds.
pub const MAXIMUM_BLOCK_WEIGHT: Weight = 2 * WEIGHT_PER_SECOND;

/// Represents the average portion of a block's weight that will be used by an
/// `on_initialize()` runtime call.
pub const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(10);

/// Represents the portion of a block that will be used by Normal extrinsics.
pub const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

/// Block number type used in Rialto.
pub type BlockNumber = u32;

/// Hash type used in Rialto.
pub type Hash = <BlakeTwo256 as HasherT>::Out;

/// The type of object that can produce hashes on Rialto.
pub type Hasher = BlakeTwo256;

/// The header type used by Rialto.
pub type Header = sp_runtime::generic::Header<BlockNumber, Hasher>;

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Public key of the chain account that may be used to verify signatures.
pub type AccountSigner = MultiSigner;

/// Balance of an account.
pub type Balance = u128;

/// An instant or duration in time.
pub type Moment = u64;

/// Index of a transaction in the parachain.
pub type Index = u32;

/// Weight-to-Fee type used by Rialto parachain.
pub type WeightToFee = IdentityFee<Balance>;

/// Rialto parachain.
#[derive(RuntimeDebug)]
pub struct RialtoParachain;

impl Chain for RialtoParachain {
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hasher = Hasher;
	type Header = Header;

	type AccountId = AccountId;
	type Balance = Balance;
	type Index = Index;
	type Signature = Signature;

	fn max_extrinsic_size() -> u32 {
		*BlockLength::get().max.get(DispatchClass::Normal)
	}

	fn max_extrinsic_weight() -> Weight {
		BlockWeights::get()
			.get(DispatchClass::Normal)
			.max_extrinsic
			.unwrap_or(Weight::MAX)
	}
}

frame_support::parameter_types! {
	pub BlockLength: limits::BlockLength =
		limits::BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub BlockWeights: limits::BlockWeights = limits::BlockWeights::builder()
		// Allowance for Normal class
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		// Allowance for Operational class
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Extra reserved space for Operational class
			weights.reserved = Some(MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		// By default Mandatory class is not limited at all.
		// This parameter is used to derive maximal size of a single extrinsic.
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
}
