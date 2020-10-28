// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Collator for the adder test parachain.

use std::{pin::Pin, sync::{Arc, Mutex}};
use test_parachain_adder::{hash_state, BlockData, HeadData, execute};
use futures::{Future, FutureExt};
use polkadot_primitives::v1::{ValidationData, PoV, Hash};
use polkadot_node_primitives::Collation;
use codec::Encode;

/// The amount we add when producing a new block.
///
/// This is a constant to make tests easily reproducible.
const ADD: u64 = 2;

/// The state of the adder parachain.
struct State {
	genesis_state: HeadData,
	last_head: HeadData,
	state: u64,
}

impl State {
	/// Init the genesis state.
	fn genesis() -> Self {
		let genesis_state = HeadData {
			number: 0,
			parent_hash: Default::default(),
			post_state: hash_state(0),
		};
		let last_head = genesis_state.clone();

		Self {
			genesis_state,
			last_head,
			state: 0,
		}
	}

	/// Advance the state and produce a new block.
	///
	/// Returns the new [`BlockData`] and the new [`HeadData`].
	fn advance(&mut self) -> (BlockData, HeadData) {
		let block = BlockData {
			state: self.state,
			add: ADD,
		};
		let new_head = execute(self.last_head.hash(), self.last_head.clone(), &block)
			.expect("Produces valid block");

		self.last_head = new_head.clone();
		self.state = self.state.wrapping_add(ADD);

		(block, new_head)
	}
}

/// The collator of the adder parachain.
pub struct Collator {
	state: Arc<Mutex<State>>,
}

impl Collator {
	/// Create a new collator instance with the state initialized as genesis.
	pub fn new() -> Self {
		Self {
			state: Arc::new(Mutex::new(State::genesis())),
		}
	}

	/// Get the SCALE encoded genesis head of the adder parachain.
	pub fn genesis_head(&self) -> Vec<u8> {
		self.state.lock().unwrap().genesis_state.encode()
	}

	/// Get the validation code of the adder parachain.
	pub fn validation_code(&self) -> &[u8] {
		test_parachain_adder::wasm_binary_unwrap()
	}

	/// Create the collation function.
	///
	/// This collation function can be plugged into the overseer to generate collations for the adder parachain.
	pub fn create_collation_function(
		&self,
	) -> Box<dyn Fn(Hash, &ValidationData) -> Pin<Box<dyn Future<Output = Option<Collation>> + Send>> + Send + Sync> {
		let state = self.state.clone();

		Box::new(move |_, _| {
			let (block_data, head_data) = state.lock().unwrap().advance();

			let collation = Collation {
				upward_messages: Vec::new(),
				new_validation_code: None,
				head_data: head_data.encode().into(),
				proof_of_validity: PoV { block_data: block_data.encode().into() },
				processed_downward_messages: 0,
			};

			async move { Some(collation) }.boxed()
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use futures::executor::block_on;
	use polkadot_parachain::{primitives::ValidationParams, wasm_executor::ExecutionMode};
	use codec::Decode;

	#[test]
	fn collator_works() {
		let collator = Collator::new();
		let collation_function = collator.create_collation_function();

		for _ in 0..5 {
			let parent_head = collator.state.lock().unwrap().last_head.clone();
			let collation = block_on(collation_function(Default::default(), &Default::default())).unwrap();
			validate_collation(&collator, parent_head, collation);
		}
	}

	fn validate_collation(collator: &Collator, parent_head: HeadData, collation: Collation) {
		let ret = polkadot_parachain::wasm_executor::validate_candidate(
			collator.validation_code(),
			ValidationParams {
				parent_head: parent_head.encode().into(),
				block_data: collation.proof_of_validity.block_data,
				relay_chain_height: 1,
				hrmp_mqc_heads: Vec::new(),
				dmq_mqc_head: Default::default(),
			},
			&ExecutionMode::InProcess,
			sp_core::testing::TaskExecutor::new(),
		).unwrap();

		let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();
		assert_eq!(collator.state.lock().unwrap().last_head, new_head);
	}
}
