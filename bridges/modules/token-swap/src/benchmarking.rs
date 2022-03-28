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

//! Token-swap pallet benchmarking.

use crate::{
	swap_account_id, target_account_at_this_chain, BridgedAccountIdOf, BridgedAccountPublicOf,
	BridgedAccountSignatureOf, BridgedBalanceOf, Call, Origin, Pallet, ThisChainBalance,
	TokenSwapCreationOf, TokenSwapOf,
};

use bp_token_swap::{TokenSwap, TokenSwapCreation, TokenSwapState, TokenSwapType};
use codec::{Decode, Encode};
use frame_benchmarking::{account, benchmarks_instance_pallet};
use frame_support::{traits::Currency, Parameter};
use frame_system::RawOrigin;
use sp_core::H256;
use sp_io::hashing::blake2_256;
use sp_runtime::traits::{Bounded, TrailingZeroInput};
use sp_std::{boxed::Box, vec::Vec};

const SEED: u32 = 0;

/// Trait that must be implemented by runtime.
pub trait Config<I: 'static>: crate::Config<I> {
	/// Initialize environment for token swap.
	fn initialize_environment();
}

benchmarks_instance_pallet! {
	where_clause {
		where
			Origin<T, I>: Into<T::Origin>,
			BridgedAccountPublicOf<T, I>: Decode + Parameter,
			BridgedAccountSignatureOf<T, I>: Decode,
	}

	//
	// Benchmarks that are used directly by the runtime.
	//

	// Benchmark `create_swap` extrinsic.
	//
	// This benchmark assumes that message is **NOT** actually sent. Instead we're using `send_message_weight`
	// from the `WeightInfoExt` trait.
	//
	// There aren't any factors that affect `create_swap` performance, so everything
	// is straightforward here.
	create_swap {
		T::initialize_environment();

		let sender = funded_account::<T, I>("source_account_at_this_chain", 0);
		let swap: TokenSwapOf<T, I> = test_swap::<T, I>(sender.clone(), true);
		let swap_creation: TokenSwapCreationOf<T, I> = test_swap_creation::<T, I>();
	}: create_swap(
		RawOrigin::Signed(sender.clone()),
		swap,
		Box::new(swap_creation)
	)
	verify {
		assert!(crate::PendingSwaps::<T, I>::contains_key(test_swap_hash::<T, I>(sender, true)));
	}

	// Benchmark `claim_swap` extrinsic with the worst possible conditions:
	//
	// * swap is locked until some block, so current block number is read.
	claim_swap {
		T::initialize_environment();

		let sender: T::AccountId = account("source_account_at_this_chain", 0, SEED);
		crate::PendingSwaps::<T, I>::insert(
			test_swap_hash::<T, I>(sender.clone(), false),
			TokenSwapState::Confirmed,
		);

		let swap: TokenSwapOf<T, I> = test_swap::<T, I>(sender.clone(), false);
		let claimer = target_account_at_this_chain::<T, I>(&swap);
		let token_swap_account = swap_account_id::<T, I>(&swap);
		T::ThisCurrency::make_free_balance_be(&token_swap_account, ThisChainBalance::<T, I>::max_value());
	}: claim_swap(RawOrigin::Signed(claimer), swap)
	verify {
		assert!(!crate::PendingSwaps::<T, I>::contains_key(test_swap_hash::<T, I>(sender, false)));
	}

	// Benchmark `cancel_swap` extrinsic with the worst possible conditions:
	//
	// * swap is locked until some block, so current block number is read.
	cancel_swap {
		T::initialize_environment();

		let sender: T::AccountId = account("source_account_at_this_chain", 0, SEED);
		crate::PendingSwaps::<T, I>::insert(
			test_swap_hash::<T, I>(sender.clone(), false),
			TokenSwapState::Failed,
		);

		let swap: TokenSwapOf<T, I> = test_swap::<T, I>(sender.clone(), false);
		let token_swap_account = swap_account_id::<T, I>(&swap);
		T::ThisCurrency::make_free_balance_be(&token_swap_account, ThisChainBalance::<T, I>::max_value());

	}: cancel_swap(RawOrigin::Signed(sender.clone()), swap)
	verify {
		assert!(!crate::PendingSwaps::<T, I>::contains_key(test_swap_hash::<T, I>(sender, false)));
	}
}

/// Returns test token swap.
fn test_swap<T: Config<I>, I: 'static>(sender: T::AccountId, is_create: bool) -> TokenSwapOf<T, I> {
	TokenSwap {
		swap_type: TokenSwapType::LockClaimUntilBlock(
			if is_create { 10u32.into() } else { 0u32.into() },
			0.into(),
		),
		source_balance_at_this_chain: source_balance_to_swap::<T, I>(),
		source_account_at_this_chain: sender,
		target_balance_at_bridged_chain: target_balance_to_swap::<T, I>(),
		target_account_at_bridged_chain: target_account_at_bridged_chain::<T, I>(),
	}
}

/// Returns test token swap hash.
fn test_swap_hash<T: Config<I>, I: 'static>(sender: T::AccountId, is_create: bool) -> H256 {
	test_swap::<T, I>(sender, is_create).using_encoded(blake2_256).into()
}

/// Returns test token swap creation params.
fn test_swap_creation<T: Config<I>, I: 'static>() -> TokenSwapCreationOf<T, I>
where
	BridgedAccountPublicOf<T, I>: Decode,
	BridgedAccountSignatureOf<T, I>: Decode,
{
	TokenSwapCreation {
		target_public_at_bridged_chain: target_public_at_bridged_chain::<T, I>(),
		swap_delivery_and_dispatch_fee: swap_delivery_and_dispatch_fee::<T, I>(),
		bridged_chain_spec_version: 0,
		bridged_currency_transfer: Vec::new(),
		bridged_currency_transfer_weight: 0,
		bridged_currency_transfer_signature: bridged_currency_transfer_signature::<T, I>(),
	}
}

/// Account that has some balance.
fn funded_account<T: Config<I>, I: 'static>(name: &'static str, index: u32) -> T::AccountId {
	let account: T::AccountId = account(name, index, SEED);
	T::ThisCurrency::make_free_balance_be(&account, ThisChainBalance::<T, I>::max_value());
	account
}

/// Currency transfer message fee.
fn swap_delivery_and_dispatch_fee<T: Config<I>, I: 'static>() -> ThisChainBalance<T, I> {
	ThisChainBalance::<T, I>::max_value() / 4u32.into()
}

/// Balance at the source chain that we're going to swap.
fn source_balance_to_swap<T: Config<I>, I: 'static>() -> ThisChainBalance<T, I> {
	ThisChainBalance::<T, I>::max_value() / 2u32.into()
}

/// Balance at the target chain that we're going to swap.
fn target_balance_to_swap<T: Config<I>, I: 'static>() -> BridgedBalanceOf<T, I> {
	BridgedBalanceOf::<T, I>::max_value() / 2u32.into()
}

/// Public key of `target_account_at_bridged_chain`.
fn target_public_at_bridged_chain<T: Config<I>, I: 'static>() -> BridgedAccountPublicOf<T, I>
where
	BridgedAccountPublicOf<T, I>: Decode,
{
	BridgedAccountPublicOf::<T, I>::decode(&mut TrailingZeroInput::zeroes())
		.expect("failed to decode `BridgedAccountPublicOf` from zeroes")
}

/// Signature of `target_account_at_bridged_chain` over message.
fn bridged_currency_transfer_signature<T: Config<I>, I: 'static>() -> BridgedAccountSignatureOf<T, I>
where
	BridgedAccountSignatureOf<T, I>: Decode,
{
	BridgedAccountSignatureOf::<T, I>::decode(&mut TrailingZeroInput::zeroes())
		.expect("failed to decode `BridgedAccountSignatureOf` from zeroes")
}

/// Account at the bridged chain that is participating in the swap.
fn target_account_at_bridged_chain<T: Config<I>, I: 'static>() -> BridgedAccountIdOf<T, I> {
	account("target_account_at_bridged_chain", 0, SEED)
}
