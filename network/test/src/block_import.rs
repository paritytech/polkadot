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

//! Testing block import logic.

use sp_consensus::ImportedAux;
use sp_consensus::import_queue::{
	import_single_block, BasicQueue, BlockImportError, BlockImportResult, IncomingBlock,
};
use polkadot_test_runtime_client::{self, prelude::*};
use polkadot_test_runtime_client::runtime::{Block, Hash};
use sp_runtime::generic::BlockId;
use super::*;

fn prepare_good_block() -> (TestClient, Hash, u64, PeerId, IncomingBlock<Block>) {
	let mut client = polkadot_test_runtime_client::new();
	let mut builder = client.new_block(Default::default()).unwrap();

	let extrinsics = polkadot_test_runtime_client::needed_extrinsics(0);

	for extrinsic in &extrinsics {
		builder.push(extrinsic.clone()).unwrap();
	}

	let block = builder.build().unwrap().block;
	client.import(BlockOrigin::File, block).unwrap();

	let (hash, number) = (client.block_hash(1).unwrap().unwrap(), 1);
	let header = client.header(&BlockId::Number(1)).unwrap();
	let justification = client.justification(&BlockId::Number(1)).unwrap();
	let peer_id = PeerId::random();
	(client, hash, number, peer_id.clone(), IncomingBlock {
		hash,
		header,
		body: Some(extrinsics),
		justification,
		origin: Some(peer_id.clone()),
		allow_missing_state: false,
		import_existing: false,
	})
}

#[test]
fn import_single_good_block_works() {
	let (_, _hash, number, peer_id, block) = prepare_good_block();

	let mut expected_aux = ImportedAux::default();
	expected_aux.is_new_best = true;

	match import_single_block(&mut polkadot_test_runtime_client::new(), BlockOrigin::File, block, &mut PassThroughVerifier::new(true)) {
		Ok(BlockImportResult::ImportedUnknown(ref num, ref aux, ref org))
			if *num == number as u32 && *aux == expected_aux && *org == Some(peer_id) => {}
		r @ _ => panic!("{:?}", r)
	}
}

#[test]
fn import_single_good_known_block_is_ignored() {
	let (mut client, _hash, number, _, block) = prepare_good_block();
	match import_single_block(&mut client, BlockOrigin::File, block, &mut PassThroughVerifier::new(true)) {
		Ok(BlockImportResult::ImportedKnown(ref n)) if *n == number as u32 => {}
		_ => panic!()
	}
}

#[test]
fn import_single_good_block_without_header_fails() {
	let (_, _, _, peer_id, mut block) = prepare_good_block();
	block.header = None;
	match import_single_block(&mut polkadot_test_runtime_client::new(), BlockOrigin::File, block, &mut PassThroughVerifier::new(true)) {
		Err(BlockImportError::IncompleteHeader(ref org)) if *org == Some(peer_id) => {}
		_ => panic!()
	}
}

#[test]
fn async_import_queue_drops() {
	let executor = sp_core::testing::TaskExecutor::new();
	// Perform this test multiple times since it exhibits non-deterministic behavior.
	for _ in 0..100 {
		let verifier = PassThroughVerifier::new(true);

		let queue = BasicQueue::new(
			verifier,
			Box::new(polkadot_test_runtime_client::new()),
			None,
			None,
			&executor,
			None
		);
		drop(queue);
	}
}
