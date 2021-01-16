// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Basic parachain that adds a number as part of its state.

const WORKER_ARGS_TEST: &[&'static str] = &["--nocapture", "validation_worker"];

use parachain::{
	primitives::{
		RelayChainBlockNumber,
		BlockData as GenericBlockData,
		HeadData as GenericHeadData,
		ValidationParams,
	},
	wasm_executor::{ValidationPool, IsolationStrategy}
};
use parity_scale_codec::{Decode, Encode};
use adder::{HeadData, BlockData, hash_state};

fn isolation_strategy() -> IsolationStrategy {
	IsolationStrategy::ExternalProcessCustomHost {
		pool: ValidationPool::new(),
		binary: std::env::current_exe().unwrap(),
		args: WORKER_ARGS_TEST.iter().map(|x| x.to_string()).collect(),
	}
}

#[test]
fn execute_good_on_parent_with_inprocess_validation() {
	let isolation_strategy = IsolationStrategy::InProcess;
	execute_good_on_parent(isolation_strategy);
}

#[test]
pub fn execute_good_on_parent_with_external_process_validation() {
	let isolation_strategy = isolation_strategy();
	execute_good_on_parent(isolation_strategy);
}

fn execute_good_on_parent(isolation_strategy: IsolationStrategy) {
	let parent_head = HeadData {
		number: 0,
		parent_hash: [0; 32],
		post_state: hash_state(0),
	};

	let block_data = BlockData {
		state: 0,
		add: 512,
	};

	let ret = parachain::wasm_executor::validate_candidate(
		adder::wasm_binary_unwrap(),
		ValidationParams {
			parent_head: GenericHeadData(parent_head.encode()),
			block_data: GenericBlockData(block_data.encode()),
			relay_chain_height: 1,
			relay_storage_root: Default::default(),
			hrmp_mqc_heads: Vec::new(),
			dmq_mqc_head: Default::default(),
		},
		&isolation_strategy,
		sp_core::testing::TaskExecutor::new(),
	).unwrap();

	let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();

	assert_eq!(new_head.number, 1);
	assert_eq!(new_head.parent_hash, parent_head.hash());
	assert_eq!(new_head.post_state, hash_state(512));
}

#[test]
fn execute_good_chain_on_parent() {
	let mut number = 0;
	let mut parent_hash = [0; 32];
	let mut last_state = 0;
	let isolation_strategy = isolation_strategy();

	for add in 0..10 {
		let parent_head = HeadData {
			number,
			parent_hash,
			post_state: hash_state(last_state),
		};

		let block_data = BlockData {
			state: last_state,
			add,
		};

		let ret = parachain::wasm_executor::validate_candidate(
			adder::wasm_binary_unwrap(),
			ValidationParams {
				parent_head: GenericHeadData(parent_head.encode()),
				block_data: GenericBlockData(block_data.encode()),
				relay_chain_height: number as RelayChainBlockNumber + 1,
				relay_storage_root: Default::default(),
				hrmp_mqc_heads: Vec::new(),
				dmq_mqc_head: Default::default(),
			},
			&isolation_strategy,
			sp_core::testing::TaskExecutor::new(),
		).unwrap();

		let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();

		assert_eq!(new_head.number, number + 1);
		assert_eq!(new_head.parent_hash, parent_head.hash());
		assert_eq!(new_head.post_state, hash_state(last_state + add));

		number += 1;
		parent_hash = new_head.hash();
		last_state += add;
	}
}

#[test]
fn execute_bad_on_parent() {
	let isolation_strategy = isolation_strategy();

	let parent_head = HeadData {
		number: 0,
		parent_hash: [0; 32],
		post_state: hash_state(0),
	};

	let block_data = BlockData {
		state: 256, // start state is wrong.
		add: 256,
	};

	let _ret = parachain::wasm_executor::validate_candidate(
		adder::wasm_binary_unwrap(),
		ValidationParams {
			parent_head: GenericHeadData(parent_head.encode()),
			block_data: GenericBlockData(block_data.encode()),
			relay_chain_height: 1,
			relay_storage_root: Default::default(),
			hrmp_mqc_heads: Vec::new(),
			dmq_mqc_head: Default::default(),
		},
		&isolation_strategy,
		sp_core::testing::TaskExecutor::new(),
	).unwrap_err();
}
