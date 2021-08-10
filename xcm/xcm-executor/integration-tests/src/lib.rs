// Copyright 2021 Parity Technologies (UK) Ltd.
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

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg(test)]

use polkadot_test_client::{
	BlockBuilderExt, ClientBlockImportExt, DefaultTestClientBuilderExt, ExecutionStrategy,
	InitPolkadotBlockBuilder, TestClientBuilder, TestClientBuilderExt,
};
use polkadot_test_service::construct_extrinsic;
use sp_runtime::{generic::BlockId, traits::Block};
use sp_state_machine::InspectState;
use xcm::latest::prelude::*;
use xcm_executor::MAX_RECURSION_LIMIT;

// This is the inflection point where the test should either fail or pass.
const MAX_RECURSION_CHECK: u32 = MAX_RECURSION_LIMIT / 2;

#[test]
fn execute_within_recursion_limit() {
	sp_tracing::try_init_simple();
	let mut client = TestClientBuilder::new()
		.set_execution_strategy(ExecutionStrategy::AlwaysWasm)
		.build();

	let mut msg = WithdrawAsset { assets: (Parent, 100).into(), effects: vec![] };
	for _ in 0..MAX_RECURSION_CHECK {
		msg = WithdrawAsset {
			assets: (Parent, 100).into(),
			effects: vec![Order::BuyExecution {
				fees: (Parent, 1).into(),
				weight: 0,
				debt: 0,
				halt_on_error: true,
				orders: vec![],
				// nest `msg` into itself on each iteration.
				instructions: vec![msg],
			}],
		};
	}

	let mut block_builder = client.init_polkadot_block_builder();

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::Call::Xcm(pallet_xcm::Call::execute(
			Box::new(msg.clone()),
			1_000_000_000,
		)),
		sp_keyring::Sr25519Keyring::Alice,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	client
		.state_at(&BlockId::Hash(block_hash))
		.expect("state should exist")
		.inspect_state(|| {
			assert!(polkadot_test_runtime::System::events().iter().any(|r| matches!(
				r.event,
				polkadot_test_runtime::Event::Xcm(pallet_xcm::Event::Attempted(Outcome::Complete(
					_
				)),),
			)));
		});
}

#[test]
fn exceed_recursion_limit() {
	sp_tracing::try_init_simple();
	let mut client = TestClientBuilder::new()
		.set_execution_strategy(ExecutionStrategy::AlwaysWasm)
		.build();

	let mut msg = WithdrawAsset { assets: (Parent, 100).into(), effects: vec![] };
	for _ in 0..(MAX_RECURSION_CHECK + 1) {
		msg = WithdrawAsset {
			assets: (Parent, 100).into(),
			effects: vec![Order::BuyExecution {
				fees: (Parent, 1).into(),
				weight: 0,
				debt: 0,
				halt_on_error: true,
				orders: vec![],
				// nest `msg` into itself on each iteration.
				instructions: vec![msg],
			}],
		};
	}

	let mut block_builder = client.init_polkadot_block_builder();

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::Call::Xcm(pallet_xcm::Call::execute(
			Box::new(msg.clone()),
			1_000_000_000,
		)),
		sp_keyring::Sr25519Keyring::Alice,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	client
		.state_at(&BlockId::Hash(block_hash))
		.expect("state should exist")
		.inspect_state(|| {
			assert!(polkadot_test_runtime::System::events().iter().any(|r| matches!(
				r.event,
				polkadot_test_runtime::Event::Xcm(pallet_xcm::Event::Attempted(
					Outcome::Incomplete(_, XcmError::RecursionLimitReached),
				)),
			)));
		});
}
