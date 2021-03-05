// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use super::*;

use crate::test_utils::{
	build_custom_header, build_genesis_header, insert_header, validator_utils::*, validators_change_receipt,
	HeaderBuilder,
};

use bp_eth_poa::{compute_merkle_root, U256};
use frame_benchmarking::benchmarks_instance;
use frame_system::RawOrigin;

benchmarks_instance! {
	// Benchmark `import_unsigned_header` extrinsic with the best possible conditions:
	// * Parent header is finalized.
	// * New header doesn't require receipts.
	// * Nothing is finalized by new header.
	// * Nothing is pruned by new header.
	import_unsigned_header_best_case {
		let n in 1..1000;

		let num_validators = 2;
		let initial_header = initialize_bench::<T, I>(num_validators);

		// prepare header to be inserted
		let header = build_custom_header(
			&validator(1),
			&initial_header,
			|mut header| {
				header.gas_limit = header.gas_limit + U256::from(n);
				header
			},
		);
	}: import_unsigned_header(RawOrigin::None, header, None)
	verify {
		let storage = BridgeStorage::<T, I>::new();
		assert_eq!(storage.best_block().0.number, 1);
		assert_eq!(storage.finalized_block().number, 0);
	}

	// Our goal with this bench is to try and see the effect that finalizing difference ranges of
	// blocks has on our import time. As such we need to make sure that we keep the number of
	// validators fixed while changing the number blocks finalized (the complexity parameter) by
	// importing the last header.
	//
	// One important thing to keep in mind is that the runtime provides a finality cache in order to
	// reduce the overhead of header finalization. However, this is only triggered every 16 blocks.
	import_unsigned_finality {
		// Our complexity parameter, n, will represent the number of blocks imported before
		// finalization.
		let n in 1..7;

		let mut storage = BridgeStorage::<T, I>::new();
		let num_validators: u32 = 2;
		let initial_header = initialize_bench::<T, I>(num_validators as usize);

		// Since we only have two validators we need to make sure the number of blocks is even to
		// make sure the right validator signs the final block
		let num_blocks = 2 * n;
		let mut headers = Vec::new();
		let mut parent = initial_header.clone();

		// Import a bunch of headers without any verification, will ensure that they're not
		// finalized prematurely
		for i in 1..=num_blocks {
			let header = HeaderBuilder::with_parent(&parent).sign_by(&validator(0));
			let id = header.compute_id();
			insert_header(&mut storage, header.clone());
			headers.push(header.clone());
			parent = header;
		}

		let last_header = headers.last().unwrap().clone();
		let last_authority = validator(1);

		// Need to make sure that the header we're going to import hasn't been inserted
		// into storage already
		let header = HeaderBuilder::with_parent(&last_header).sign_by(&last_authority);
	}: import_unsigned_header(RawOrigin::None, header, None)
	verify {
		let storage = BridgeStorage::<T, I>::new();
		assert_eq!(storage.best_block().0.number, (num_blocks + 1) as u64);
		assert_eq!(storage.finalized_block().number, num_blocks as u64);
	}

	// Basically the exact same as `import_unsigned_finality` but with a different range for the
	// complexity parameter. In this bench we use a larger range of blocks to see how performance
	// changes when the finality cache kicks in (>16 blocks).
	import_unsigned_finality_with_cache {
		// Our complexity parameter, n, will represent the number of blocks imported before
		// finalization.
		let n in 7..100;

		let mut storage = BridgeStorage::<T, I>::new();
		let num_validators: u32 = 2;
		let initial_header = initialize_bench::<T, I>(num_validators as usize);

		// Since we only have two validators we need to make sure the number of blocks is even to
		// make sure the right validator signs the final block
		let num_blocks = 2 * n;
		let mut headers = Vec::new();
		let mut parent = initial_header.clone();

		// Import a bunch of headers without any verification, will ensure that they're not
		// finalized prematurely
		for i in 1..=num_blocks {
			let header = HeaderBuilder::with_parent(&parent).sign_by(&validator(0));
			let id = header.compute_id();
			insert_header(&mut storage, header.clone());
			headers.push(header.clone());
			parent = header;
		}

		let last_header = headers.last().unwrap().clone();
		let last_authority = validator(1);

		// Need to make sure that the header we're going to import hasn't been inserted
		// into storage already
		let header = HeaderBuilder::with_parent(&last_header).sign_by(&last_authority);
	}: import_unsigned_header(RawOrigin::None, header, None)
	verify {
		let storage = BridgeStorage::<T, I>::new();
		assert_eq!(storage.best_block().0.number, (num_blocks + 1) as u64);
		assert_eq!(storage.finalized_block().number, num_blocks as u64);
	}

	// A block import may trigger a pruning event, which adds extra work to the import progress.
	// In this bench we trigger a pruning event in order to see how much extra time is spent by the
	// runtime dealing with it. In the Ethereum Pallet, we're limited pruning to eight blocks in a
	// single import, as dictated by MAX_BLOCKS_TO_PRUNE_IN_SINGLE_IMPORT.
	import_unsigned_pruning {
		let n in 1..MAX_BLOCKS_TO_PRUNE_IN_SINGLE_IMPORT as u32;

		let mut storage = BridgeStorage::<T, I>::new();

		let num_validators = 3;
		let initial_header = initialize_bench::<T, I>(num_validators as usize);
		let validators = validators(num_validators);

		// Want to prune eligible blocks between [0, n)
		BlocksToPrune::<I>::put(PruningRange {
			oldest_unpruned_block: 0,
			oldest_block_to_keep: n as u64,
		});

		let mut parent = initial_header;
		for i in 1..=n {
			let header = HeaderBuilder::with_parent(&parent).sign_by_set(&validators);
			let id = header.compute_id();
			insert_header(&mut storage, header.clone());
			parent = header;
		}

		let header = HeaderBuilder::with_parent(&parent).sign_by_set(&validators);
	}: import_unsigned_header(RawOrigin::None, header, None)
	verify {
		let storage = BridgeStorage::<T, I>::new();
		let max_pruned: u64 = (n - 1) as _;
		assert_eq!(storage.best_block().0.number, (n + 1) as u64);
		assert!(HeadersByNumber::<I>::get(&0).is_none());
		assert!(HeadersByNumber::<I>::get(&max_pruned).is_none());
	}

	// The goal of this bench is to import a block which contains a transaction receipt. The receipt
	// will contain a validator set change. Verifying the receipt root is an expensive operation to
	// do, which is why we're interested in benchmarking it.
	import_unsigned_with_receipts {
		let n in 1..100;

		let mut storage = BridgeStorage::<T, I>::new();

		let num_validators = 1;
		let initial_header = initialize_bench::<T, I>(num_validators as usize);

		let mut receipts = vec![];
		for i in 1..=n {
			let receipt = validators_change_receipt(Default::default());
			receipts.push(receipt)
		}
		let encoded_receipts = receipts.iter().map(|r| r.rlp());

		// We need this extra header since this is what signals a validator set transition. This
		// will ensure that the next header is within the "Contract" window
		let header1 = HeaderBuilder::with_parent(&initial_header).sign_by(&validator(0));
		insert_header(&mut storage, header1.clone());

		let header = build_custom_header(
			&validator(0),
			&header1,
			|mut header| {
				// Logs Bloom signals a change in validator set
				header.log_bloom = (&[0xff; 256]).into();
				header.receipts_root = compute_merkle_root(encoded_receipts);
				header
			},
		);
	}: import_unsigned_header(RawOrigin::None, header, Some(receipts))
	verify {
		let storage = BridgeStorage::<T, I>::new();
		assert_eq!(storage.best_block().0.number, 2);
	}
}

fn initialize_bench<T: Config<I>, I: Instance>(num_validators: usize) -> AuraHeader {
	// Initialize storage with some initial header
	let initial_header = build_genesis_header(&validator(0));
	let initial_difficulty = initial_header.difficulty;
	let initial_validators = validators_addresses(num_validators as usize);

	initialize_storage::<T, I>(&initial_header, initial_difficulty, &initial_validators);

	initial_header
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{run_test, TestRuntime};
	use frame_support::assert_ok;

	#[test]
	fn insert_unsigned_header_best_case() {
		run_test(1, |_| {
			assert_ok!(test_benchmark_import_unsigned_header_best_case::<TestRuntime>());
		});
	}

	#[test]
	fn insert_unsigned_header_finality() {
		run_test(1, |_| {
			assert_ok!(test_benchmark_import_unsigned_finality::<TestRuntime>());
		});
	}

	#[test]
	fn insert_unsigned_header_finality_with_cache() {
		run_test(1, |_| {
			assert_ok!(test_benchmark_import_unsigned_finality_with_cache::<TestRuntime>());
		});
	}

	#[test]
	fn insert_unsigned_header_pruning() {
		run_test(1, |_| {
			assert_ok!(test_benchmark_import_unsigned_pruning::<TestRuntime>());
		});
	}

	#[test]
	fn insert_unsigned_header_receipts() {
		run_test(1, |_| {
			assert_ok!(test_benchmark_import_unsigned_with_receipts::<TestRuntime>());
		});
	}
}
