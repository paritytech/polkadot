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

//! Storage Proof Checker fuzzer.

#![warn(missing_docs)]

use honggfuzz::fuzz;
// Logic for checking Substrate storage proofs.

use sp_core::{Blake2Hasher, H256};
use sp_state_machine::{backend::Backend, prove_read, InMemoryBackend};
use sp_std::vec::Vec;
use sp_trie::StorageProof;
use std::collections::HashMap;

fn craft_known_storage_proof(input_vec: Vec<(Vec<u8>, Vec<u8>)>) -> (H256, StorageProof) {
	let storage_proof_vec = vec![(
		None,
		input_vec.iter().map(|x| (x.0.clone(), Some(x.1.clone()))).collect(),
	)];
	log::info!("Storage proof vec {:?}", storage_proof_vec);
	let backend = <InMemoryBackend<Blake2Hasher>>::from(storage_proof_vec);
	let root = backend.storage_root(std::iter::empty()).0;
	let vector_element_proof = StorageProof::new(
		prove_read(backend, input_vec.iter().map(|x| x.0.as_slice()))
			.unwrap()
			.iter_nodes()
			.collect(),
	);
	(root, vector_element_proof)
}

fn transform_into_unique(input_vec: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<(Vec<u8>, Vec<u8>)> {
	let mut output_hashmap = HashMap::new();
	let mut output_vec = Vec::new();
	for key_value_pair in input_vec.clone() {
		output_hashmap.insert(key_value_pair.0, key_value_pair.1); //Only 1 value per key
	}
	for (key, val) in output_hashmap.iter() {
		output_vec.push((key.clone(), val.clone()));
	}
	output_vec
}

fn run_fuzzer() {
	fuzz!(|input_vec: Vec<(Vec<u8>, Vec<u8>)>| {
		if input_vec.is_empty() {
			return;
		}
		let unique_input_vec = transform_into_unique(input_vec);
		let (root, craft_known_storage_proof) = craft_known_storage_proof(unique_input_vec.clone());
		let checker = <bp_runtime::StorageProofChecker<Blake2Hasher>>::new(root, craft_known_storage_proof)
			.expect("Valid proof passed; qed");
		for key_value_pair in unique_input_vec {
			log::info!("Reading value for pair {:?}", key_value_pair);
			assert_eq!(
				checker.read_value(&key_value_pair.0),
				Ok(Some(key_value_pair.1.clone()))
			);
		}
	})
}

fn main() {
	env_logger::init();

	loop {
		run_fuzzer();
	}
}
