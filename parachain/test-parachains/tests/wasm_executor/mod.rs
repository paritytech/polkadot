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

//! Basic parachain that adds a number as part of its state.

use crate::adder;
use parachain::{
	primitives::{BlockData, ValidationParams},
	wasm_executor::{ValidationError, InvalidCandidate, EXECUTION_TIMEOUT_SEC},
};

#[test]
fn terminates_on_timeout() {
	let pool = parachain::wasm_executor::ValidationPool::new();

	let result = parachain::wasm_executor::validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_chain_height: 1,
			hrmp_mqc_heads: Vec::new(),
		},
		parachain::wasm_executor::ExecutionMode::RemoteTest(&pool),
		sp_core::testing::TaskExecutor::new(),
	);
	match result {
		Err(ValidationError::InvalidCandidate(InvalidCandidate::Timeout)) => {},
		r => panic!("{:?}", r),
	}

	// check that another parachain can validate normaly
	adder::execute_good_on_parent();
}

#[test]
fn parallel_execution() {
	let pool = parachain::wasm_executor::ValidationPool::new();

	let start = std::time::Instant::now();

	let pool2 = pool.clone();
	let thread = std::thread::spawn(move ||
		parachain::wasm_executor::validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_chain_height: 1,
			hrmp_mqc_heads: Vec::new(),
		},
		parachain::wasm_executor::ExecutionMode::RemoteTest(&pool2),
		sp_core::testing::TaskExecutor::new(),
	).ok());
	let _ = parachain::wasm_executor::validate_candidate(
		halt::wasm_binary_unwrap(),
		ValidationParams {
			block_data: BlockData(Vec::new()),
			parent_head: Default::default(),
			relay_chain_height: 1,
			hrmp_mqc_heads: Vec::new(),
		},
		parachain::wasm_executor::ExecutionMode::RemoteTest(&pool),
		sp_core::testing::TaskExecutor::new(),
	);
	thread.join().unwrap();
	// total time should be < 2 x EXECUTION_TIMEOUT_SEC
	assert!(
		std::time::Instant::now().duration_since(start)
		< std::time::Duration::from_secs(EXECUTION_TIMEOUT_SEC * 2)
	);
}
