// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Collator for the `Undying` test parachain.

use futures::channel::oneshot;
use futures_timer::Delay;
use parity_scale_codec::{Decode, Encode};
use polkadot_node_primitives::{
	maybe_compress_pov, Collation, CollationResult, CollationSecondedSignal, CollatorFn,
	MaybeCompressedPoV, PoV, Statement,
};
use polkadot_primitives::v2::{CollatorId, CollatorPair, Hash};
use sp_core::Pair;
use std::{
	collections::HashMap,
	sync::{
		atomic::{AtomicU32, Ordering},
		Arc, Mutex,
	},
	time::Duration,
};
use test_parachain_undying::{execute, hash_state, BlockData, GraveyardState, HeadData};

/// Default PoV size which also drives state size.
const DEFAULT_POV_SIZE: usize = 1000;
/// Default PVF time complexity - 1 signature per block.
const DEFAULT_PVF_COMPLEXITY: u32 = 1;

/// Calculates the head and state for the block with the given `number`.
fn calculate_head_and_state_for_number(
	number: u64,
	graveyard_size: usize,
	pvf_complexity: u32,
) -> (HeadData, GraveyardState) {
	let index = 0u64;
	let mut graveyard = vec![0u8; graveyard_size * graveyard_size];
	let zombies = 0;
	let seal = [0u8; 32];

	// Ensure a larger compressed PoV.
	graveyard.iter_mut().enumerate().for_each(|(i, grave)| {
		*grave = i as u8;
	});

	let mut state = GraveyardState { index, graveyard, zombies, seal };
	let mut head =
		HeadData { number: 0, parent_hash: Hash::default().into(), post_state: hash_state(&state) };

	while head.number < number {
		let block = BlockData { state, tombstones: 1_000, iterations: pvf_complexity };
		let (new_head, new_state) =
			execute(head.hash(), head.clone(), block).expect("Produces valid block");
		head = new_head;
		state = new_state;
	}

	(head, state)
}

/// The state of the undying parachain.
struct State {
	// We need to keep these around until the including relay chain blocks are finalized.
	// This is because disputes can trigger reverts up to last finalized block, so we
	// want that state to collate on older relay chain heads.
	head_to_state: HashMap<Arc<HeadData>, GraveyardState>,
	number_to_head: HashMap<u64, Arc<HeadData>>,
	/// Block number of the best block.
	best_block: u64,
	/// PVF time complexity.
	pvf_complexity: u32,
	/// Defines the state size (Vec<u8>). Our PoV includes the entire state so this value will
	/// drive the PoV size.
	/// Important note: block execution heavily clones this state, so something like 300.000 is
	/// the max value here, otherwise we'll get OOM during wasm execution.
	/// TODO: Implement a static state, and use `ballast` to inflate the PoV size. This way
	/// we can just discard the `ballast` before processing the block.
	graveyard_size: usize,
}

impl State {
	/// Init the genesis state.
	fn genesis(graveyard_size: usize, pvf_complexity: u32) -> Self {
		let index = 0u64;
		let mut graveyard = vec![0u8; graveyard_size * graveyard_size];
		let zombies = 0;
		let seal = [0u8; 32];

		// Ensure a larger compressed PoV.
		graveyard.iter_mut().enumerate().for_each(|(i, grave)| {
			*grave = i as u8;
		});

		let state = GraveyardState { index, graveyard, zombies, seal };

		let head_data =
			HeadData { number: 0, parent_hash: Default::default(), post_state: hash_state(&state) };
		let head_data = Arc::new(head_data);

		Self {
			head_to_state: vec![(head_data.clone(), state.clone())].into_iter().collect(),
			number_to_head: vec![(0, head_data)].into_iter().collect(),
			best_block: 0,
			pvf_complexity,
			graveyard_size,
		}
	}

	/// Advance the state and produce a new block based on the given `parent_head`.
	///
	/// Returns the new [`BlockData`] and the new [`HeadData`].
	fn advance(&mut self, parent_head: HeadData) -> (BlockData, HeadData) {
		self.best_block = parent_head.number;

		let state = if let Some(head_data) = self.number_to_head.get(&self.best_block) {
			self.head_to_state.get(head_data).cloned().unwrap_or_else(|| {
				calculate_head_and_state_for_number(
					parent_head.number,
					self.graveyard_size,
					self.pvf_complexity,
				)
				.1
			})
		} else {
			let (_, state) = calculate_head_and_state_for_number(
				parent_head.number,
				self.graveyard_size,
				self.pvf_complexity,
			);
			state
		};

		// Start with prev state and transaction to execute (place 1000 tombstones).
		let block = BlockData { state, tombstones: 1000, iterations: self.pvf_complexity };

		let (new_head, new_state) =
			execute(parent_head.hash(), parent_head, block.clone()).expect("Produces valid block");

		let new_head_arc = Arc::new(new_head.clone());

		self.head_to_state.insert(new_head_arc.clone(), new_state);
		self.number_to_head.insert(new_head.number, new_head_arc);

		(block, new_head)
	}
}

/// The collator of the undying parachain.
pub struct Collator {
	state: Arc<Mutex<State>>,
	key: CollatorPair,
	seconded_collations: Arc<AtomicU32>,
}

impl Default for Collator {
	fn default() -> Self {
		Self::new(DEFAULT_POV_SIZE, DEFAULT_PVF_COMPLEXITY)
	}
}

impl Collator {
	/// Create a new collator instance with the state initialized from genesis and `pov_size`
	/// parameter. The same parameter needs to be passed when exporting the genesis state.
	pub fn new(pov_size: usize, pvf_complexity: u32) -> Self {
		let graveyard_size = ((pov_size / std::mem::size_of::<u8>()) as f64).sqrt().ceil() as usize;

		log::info!(
			"PoV target size: {} bytes. Graveyard size: ({} x {})",
			pov_size,
			graveyard_size,
			graveyard_size
		);

		log::info!("PVF time complexity: {}", pvf_complexity);

		Self {
			state: Arc::new(Mutex::new(State::genesis(graveyard_size, pvf_complexity))),
			key: CollatorPair::generate().0,
			seconded_collations: Arc::new(AtomicU32::new(0)),
		}
	}

	/// Get the SCALE encoded genesis head of the parachain.
	pub fn genesis_head(&self) -> Vec<u8> {
		self.state
			.lock()
			.unwrap()
			.number_to_head
			.get(&0)
			.expect("Genesis header exists")
			.encode()
	}

	/// Get the validation code of the undying parachain.
	pub fn validation_code(&self) -> &[u8] {
		test_parachain_undying::wasm_binary_unwrap()
	}

	/// Get the collator key.
	pub fn collator_key(&self) -> CollatorPair {
		self.key.clone()
	}

	/// Get the collator id.
	pub fn collator_id(&self) -> CollatorId {
		self.key.public()
	}

	/// Create the collation function.
	///
	/// This collation function can be plugged into the overseer to generate collations for the undying parachain.
	pub fn create_collation_function(
		&self,
		spawner: impl SpawnNamed + Clone + 'static,
	) -> CollatorFn {
		use futures::FutureExt as _;

		let state = self.state.clone();
		let seconded_collations = self.seconded_collations.clone();

		Box::new(move |relay_parent, validation_data| {
			let parent = HeadData::decode(&mut &validation_data.parent_head.0[..])
				.expect("Decodes parent head");

			let (block_data, head_data) = state.lock().unwrap().advance(parent);

			log::info!(
				"created a new collation on relay-parent({}): {:?}",
				relay_parent,
				head_data,
			);

			// The pov is the actually the initial state and the transactions.
			let pov = PoV { block_data: block_data.encode().into() };

			let collation = Collation {
				upward_messages: Vec::new(),
				horizontal_messages: Vec::new(),
				new_validation_code: None,
				head_data: head_data.encode().into(),
				proof_of_validity: MaybeCompressedPoV::Raw(pov.clone()),
				processed_downward_messages: 0,
				hrmp_watermark: validation_data.relay_parent_number,
			};

			log::info!("Raw PoV size for collation: {} bytes", pov.block_data.0.len(),);
			let compressed_pov = maybe_compress_pov(pov);

			log::info!(
				"Compressed PoV size for collation: {} bytes",
				compressed_pov.block_data.0.len(),
			);

			let (result_sender, recv) = oneshot::channel::<CollationSecondedSignal>();
			let seconded_collations = seconded_collations.clone();
			spawner.spawn(
				"undying-collator-seconded",
				None,
				async move {
					if let Ok(res) = recv.await {
						if !matches!(
							res.statement.payload(),
							Statement::Seconded(s) if s.descriptor.pov_hash == compressed_pov.hash(),
						) {
							log::error!(
								"Seconded statement should match our collation: {:?}",
								res.statement.payload()
							);
							std::process::exit(-1);
						}

						seconded_collations.fetch_add(1, Ordering::Relaxed);
					}
				}
				.boxed(),
			);

			async move { Some(CollationResult { collation, result_sender: Some(result_sender) }) }
				.boxed()
		})
	}

	/// Wait until `blocks` are built and enacted.
	pub async fn wait_for_blocks(&self, blocks: u64) {
		let start_block = self.state.lock().unwrap().best_block;
		loop {
			Delay::new(Duration::from_secs(1)).await;

			let current_block = self.state.lock().unwrap().best_block;

			if start_block + blocks <= current_block {
				return
			}
		}
	}

	/// Wait until `seconded` collations of this collator are seconded by a parachain validator.
	///
	/// The internal counter isn't de-duplicating the collations when counting the number of seconded collations. This
	/// means when one collation is seconded by X validators, we record X seconded messages.
	pub async fn wait_for_seconded_collations(&self, seconded: u32) {
		let seconded_collations = self.seconded_collations.clone();
		loop {
			Delay::new(Duration::from_secs(1)).await;

			if seconded <= seconded_collations.load(Ordering::Relaxed) {
				return
			}
		}
	}
}

use sp_core::traits::SpawnNamed;

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::block_on;
	use polkadot_parachain::primitives::{ValidationParams, ValidationResult};
	use polkadot_primitives::v2::{Hash, PersistedValidationData};

	#[test]
	fn collator_works() {
		let spawner = sp_core::testing::TaskExecutor::new();
		let collator = Collator::new(1_000, 1);
		let collation_function = collator.create_collation_function(spawner);

		for i in 0..5 {
			let parent_head =
				collator.state.lock().unwrap().number_to_head.get(&i).unwrap().clone();

			let validation_data = PersistedValidationData {
				parent_head: parent_head.encode().into(),
				..Default::default()
			};

			let collation =
				block_on(collation_function(Default::default(), &validation_data)).unwrap();
			validate_collation(&collator, (*parent_head).clone(), collation.collation);
		}
	}

	fn validate_collation(collator: &Collator, parent_head: HeadData, collation: Collation) {
		use polkadot_node_core_pvf::testing::validate_candidate;

		let block_data = match collation.proof_of_validity {
			MaybeCompressedPoV::Raw(pov) => pov.block_data,
			MaybeCompressedPoV::Compressed(_) => panic!("Only works with uncompressed povs"),
		};

		let ret_buf = validate_candidate(
			collator.validation_code(),
			&ValidationParams {
				parent_head: parent_head.encode().into(),
				block_data,
				relay_parent_number: 1,
				relay_parent_storage_root: Hash::zero(),
			}
			.encode(),
		)
		.unwrap();
		let ret = ValidationResult::decode(&mut &ret_buf[..]).unwrap();

		let new_head = HeadData::decode(&mut &ret.head_data.0[..]).unwrap();
		assert_eq!(
			**collator
				.state
				.lock()
				.unwrap()
				.number_to_head
				.get(&(parent_head.number + 1))
				.unwrap(),
			new_head
		);
	}

	#[test]
	fn advance_to_state_when_parent_head_is_missing() {
		let collator = Collator::new(1_000, 1);
		let graveyard_size = collator.state.lock().unwrap().graveyard_size;

		let mut head = calculate_head_and_state_for_number(10, graveyard_size, 1).0;

		for i in 1..10 {
			head = collator.state.lock().unwrap().advance(head).1;
			assert_eq!(10 + i, head.number);
		}

		let collator = Collator::new(1_000, 1);
		let mut second_head = collator
			.state
			.lock()
			.unwrap()
			.number_to_head
			.get(&0)
			.cloned()
			.unwrap()
			.as_ref()
			.clone();

		for _ in 1..20 {
			second_head = collator.state.lock().unwrap().advance(second_head.clone()).1;
		}

		assert_eq!(second_head, head);
	}
}
