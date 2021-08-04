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

//! Benchmarks for the GRANDPA Pallet.
//!
//! The main dispatchable for the GRANDPA pallet is `submit_finality_proof`, so these benchmarks are
//! based around that. There are to main factors which affect finality proof verification:
//!
//! 1. The number of `votes-ancestries` in the justification
//! 2. The number of `pre-commits` in the justification
//!
//! Vote ancestries are the headers between (`finality_target`, `head_of_chain`], where
//! `header_of_chain` is a decendant of `finality_target`.
//!
//! Pre-commits are messages which are signed by validators at the head of the chain they think is
//! the best.
//!
//! Consider the following:
//!
//!   / [B'] <- [C']
//! [A] <- [B] <- [C]
//!
//! The common ancestor of both forks is block A, so this is what GRANDPA will finalize. In order to
//! verify this we will have vote ancestries of [B, C, B', C'] and pre-commits [C, C'].
//!
//! Note that the worst case scenario here would be a justification where each validator has it's
//! own fork which is `SESSION_LENGTH` blocks long.

use crate::*;

use bp_test_utils::{
	accounts, make_justification_for_header, JustificationGeneratorParams, TEST_GRANDPA_ROUND, TEST_GRANDPA_SET_ID,
};
use frame_benchmarking::{benchmarks_instance_pallet, whitelisted_caller};
use frame_support::traits::Get;
use frame_system::RawOrigin;
use sp_finality_grandpa::AuthorityId;
use sp_runtime::traits::Zero;
use sp_std::vec::Vec;

// The maximum number of vote ancestries to include in a justification.
//
// In practice this would be limited by the session length (number of blocks a single authority set
// can produce) of a given chain.
const MAX_VOTE_ANCESTRIES: u32 = 1000;

// The maximum number of pre-commits to include in a justification. In practice this scales with the
// number of validators.
const MAX_VALIDATOR_SET_SIZE: u32 = 1024;

/// Returns number of first header to be imported.
///
/// Since we boostrap the pallet with `HeadersToKeep` already imported headers,
/// this function computes the next expected header number to import.
fn header_number<T: Config<I>, I: 'static, N: From<u32>>() -> N {
	(T::HeadersToKeep::get() + 1).into()
}

/// Prepare header and its justification to submit using `submit_finality_proof`.
fn prepare_benchmark_data<T: Config<I>, I: 'static>(
	precommits: u32,
	ancestors: u32,
) -> (BridgedHeader<T, I>, GrandpaJustification<BridgedHeader<T, I>>) {
	let authority_list = accounts(precommits as u16)
		.iter()
		.map(|id| (AuthorityId::from(*id), 1))
		.collect::<Vec<_>>();

	let init_data = InitializationData {
		header: bp_test_utils::test_header(Zero::zero()),
		authority_list,
		set_id: TEST_GRANDPA_SET_ID,
		is_halted: false,
	};

	bootstrap_bridge::<T, I>(init_data);

	let header: BridgedHeader<T, I> = bp_test_utils::test_header(header_number::<T, I, _>());
	let params = JustificationGeneratorParams {
		header: header.clone(),
		round: TEST_GRANDPA_ROUND,
		set_id: TEST_GRANDPA_SET_ID,
		authorities: accounts(precommits as u16).iter().map(|k| (*k, 1)).collect::<Vec<_>>(),
		ancestors,
		forks: 1,
	};
	let justification = make_justification_for_header(params);
	(header, justification)
}

benchmarks_instance_pallet! {
	// This is the "gold standard" benchmark for this extrinsic, and it's what should be used to
	// annotate the weight in the pallet.
	submit_finality_proof {
		let p in 1..MAX_VALIDATOR_SET_SIZE;
		let v in 1..MAX_VOTE_ANCESTRIES;
		let caller: T::AccountId = whitelisted_caller();
		let (header, justification) = prepare_benchmark_data::<T, I>(p, v);
	}: submit_finality_proof(RawOrigin::Signed(caller), header, justification)
	verify {
		let header: BridgedHeader<T, I> = bp_test_utils::test_header(header_number::<T, I, _>());
		let expected_hash = header.hash();

		assert_eq!(<BestFinalized<T, I>>::get(), expected_hash);
		assert!(<ImportedHeaders<T, I>>::contains_key(expected_hash));
	}
}
