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

use parachain::wasm_executor::{ValidationPool, ExecutionMode};
use parachain::primitives::{
	BlockData as GenericBlockData,
	HeadData as GenericHeadData,
	ValidationParams, ValidationCode,
};
use codec::{Decode, Encode};
use code_upgrader::{hash_state, HeadData, BlockData, State};

const WORKER_ARGS_TEST: &[&'static str] = &["--nocapture", "validation_worker"];

fn execution_mode() -> ExecutionMode {
	ExecutionMode::ExternalProcessCustomHost {
		pool: ValidationPool::new(),
		binary: std::env::current_exe().unwrap(),
		args: WORKER_ARGS_TEST.iter().map(|x| x.to_string()).collect(),
	}
}

#[test]
pub fn execute_good_no_upgrade() {
	let execution_mode = execution_mode();

	let parent_head = HeadData {
		number: 0,
		parent_hash: [0; 32],
		post_state: hash_state(&State::default()),
	};

	let block_data = BlockData {
		state: State::default(),
		new_validation_code: None,
	};

	let ret = parachain::wasm_executor::validate_candidate(
		code_upgrader::wasm_binary_unwrap(),
		ValidationParams {
			parent_head: GenericHeadData(parent_head.encode()),
			block_data: GenericBlockData(block_data.encode()),
			max_code_size: 1024,
			max_head_data_size: 1024,
			relay_chain_height: 1,
			code_upgrade_allowed: None,
		},
		&execution_mode,
		sp_core::testing::TaskExecutor::new(),
	).unwrap();

	let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();

	assert!(ret.new_validation_code.is_none());
	assert_eq!(new_head.number, 1);
	assert_eq!(new_head.parent_hash, parent_head.hash());
	assert_eq!(new_head.post_state, hash_state(&State::default()));
}

#[test]
pub fn execute_good_with_upgrade() {
	let execution_mode = execution_mode();

	let parent_head = HeadData {
		number: 0,
		parent_hash: [0; 32],
		post_state: hash_state(&State::default()),
	};

	let block_data = BlockData {
		state: State::default(),
		new_validation_code: Some(ValidationCode(vec![1, 2, 3])),
	};

	let ret = parachain::wasm_executor::validate_candidate(
		code_upgrader::wasm_binary_unwrap(),
		ValidationParams {
			parent_head: GenericHeadData(parent_head.encode()),
			block_data: GenericBlockData(block_data.encode()),
			max_code_size: 1024,
			max_head_data_size: 1024,
			relay_chain_height: 1,
			code_upgrade_allowed: Some(20),
		},
		&execution_mode,
		sp_core::testing::TaskExecutor::new(),
	).unwrap();

	let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();

	assert_eq!(ret.new_validation_code.unwrap(), ValidationCode(vec![1, 2, 3]));
	assert_eq!(new_head.number, 1);
	assert_eq!(new_head.parent_hash, parent_head.hash());
	assert_eq!(
		new_head.post_state,
		hash_state(&State {
			code: ValidationCode::default(),
			pending_code: Some((ValidationCode(vec![1, 2, 3]), 20)),
		}),
	);
}

#[test]
#[should_panic]
pub fn code_upgrade_not_allowed() {
	let execution_mode = execution_mode();

	let parent_head = HeadData {
		number: 0,
		parent_hash: [0; 32],
		post_state: hash_state(&State::default()),
	};

	let block_data = BlockData {
		state: State::default(),
		new_validation_code: Some(ValidationCode(vec![1, 2, 3])),
	};

	parachain::wasm_executor::validate_candidate(
		code_upgrader::wasm_binary_unwrap(),
		ValidationParams {
			parent_head: GenericHeadData(parent_head.encode()),
			block_data: GenericBlockData(block_data.encode()),
			max_code_size: 1024,
			max_head_data_size: 1024,
			relay_chain_height: 1,
			code_upgrade_allowed: None,
		},
		&execution_mode,
		sp_core::testing::TaskExecutor::new(),
	).unwrap();
}

#[test]
pub fn applies_code_upgrade_after_delay() {
	let execution_mode = execution_mode();

	let (new_head, state) = {
		let parent_head = HeadData {
			number: 0,
			parent_hash: [0; 32],
			post_state: hash_state(&State::default()),
		};

		let block_data = BlockData {
			state: State::default(),
			new_validation_code: Some(ValidationCode(vec![1, 2, 3])),
		};

		let ret = parachain::wasm_executor::validate_candidate(
			code_upgrader::wasm_binary_unwrap(),
			ValidationParams {
				parent_head: GenericHeadData(parent_head.encode()),
				block_data: GenericBlockData(block_data.encode()),
				max_code_size: 1024,
				max_head_data_size: 1024,
				relay_chain_height: 1,
				code_upgrade_allowed: Some(2),
			},
			&execution_mode,
			sp_core::testing::TaskExecutor::new(),
		).unwrap();

		let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();

		let parent_hash = parent_head.hash();
		let state = State {
			code: ValidationCode::default(),
			pending_code: Some((ValidationCode(vec![1, 2, 3]), 2)),
		};
		assert_eq!(ret.new_validation_code.unwrap(), ValidationCode(vec![1, 2, 3]));
		assert_eq!(new_head.number, 1);
		assert_eq!(new_head.parent_hash, parent_hash);
		assert_eq!(new_head.post_state, hash_state(&state));

		(new_head, state)
	};

	{
		let parent_head = new_head;
		let block_data = BlockData {
			state,
			new_validation_code: None,
		};

		let ret = parachain::wasm_executor::validate_candidate(
			code_upgrader::wasm_binary_unwrap(),
			ValidationParams {
				parent_head: GenericHeadData(parent_head.encode()),
				block_data: GenericBlockData(block_data.encode()),
				max_code_size: 1024,
				max_head_data_size: 1024,
				relay_chain_height: 2,
				code_upgrade_allowed: None,
			},
			&execution_mode,
			sp_core::testing::TaskExecutor::new(),
		).unwrap();

		let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();

		assert!(ret.new_validation_code.is_none());
		assert_eq!(new_head.number, 2);
		assert_eq!(new_head.parent_hash, parent_head.hash());
		assert_eq!(
			new_head.post_state,
			hash_state(&State {
				code: ValidationCode(vec![1, 2, 3]),
				pending_code: None,
			}),
		);
	}
}
