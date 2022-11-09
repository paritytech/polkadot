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
use polkadot_test_runtime::pallet_test_notifier;
use polkadot_test_service::construct_extrinsic;
use sp_runtime::traits::Block;
use sp_state_machine::InspectState;
use xcm::{latest::prelude::*, VersionedResponse, VersionedXcm};

#[test]
fn basic_buy_fees_message_executes() {
	sp_tracing::try_init_simple();
	let mut client = TestClientBuilder::new()
		.set_execution_strategy(ExecutionStrategy::AlwaysWasm)
		.build();

	let msg = Xcm(vec![
		WithdrawAsset((Parent, 100).into()),
		BuyExecution { fees: (Parent, 1).into(), weight_limit: Unlimited },
		DepositAsset { assets: Wild(All), max_assets: 1, beneficiary: Parent.into() },
	]);

	let mut block_builder = client.init_polkadot_block_builder();

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::RuntimeCall::Xcm(pallet_xcm::Call::execute {
			message: Box::new(VersionedXcm::from(msg)),
			max_weight: 1_000_000_000,
		}),
		sp_keyring::Sr25519Keyring::Alice,
		0,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	client.state_at(block_hash).expect("state should exist").inspect_state(|| {
		assert!(polkadot_test_runtime::System::events().iter().any(|r| matches!(
			r.event,
			polkadot_test_runtime::RuntimeEvent::Xcm(pallet_xcm::Event::Attempted(
				Outcome::Complete(_)
			)),
		)));
	});
}

#[test]
fn query_response_fires() {
	use pallet_test_notifier::Event::*;
	use pallet_xcm::QueryStatus;
	use polkadot_test_runtime::RuntimeEvent::TestNotifier;

	sp_tracing::try_init_simple();
	let mut client = TestClientBuilder::new()
		.set_execution_strategy(ExecutionStrategy::AlwaysWasm)
		.build();

	let mut block_builder = client.init_polkadot_block_builder();

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::RuntimeCall::TestNotifier(
			pallet_test_notifier::Call::prepare_new_query {},
		),
		sp_keyring::Sr25519Keyring::Alice,
		0,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	let mut query_id = None;
	client.state_at(block_hash).expect("state should exist").inspect_state(|| {
		for r in polkadot_test_runtime::System::events().iter() {
			match r.event {
				TestNotifier(QueryPrepared(q)) => query_id = Some(q),
				_ => (),
			}
		}
	});
	let query_id = query_id.unwrap();

	let mut block_builder = client.init_polkadot_block_builder();

	let response = Response::ExecutionResult(None);
	let max_weight = 1_000_000;
	let msg = Xcm(vec![QueryResponse { query_id, response, max_weight }]);
	let msg = Box::new(VersionedXcm::from(msg));

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::RuntimeCall::Xcm(pallet_xcm::Call::execute {
			message: msg,
			max_weight: 1_000_000_000,
		}),
		sp_keyring::Sr25519Keyring::Alice,
		1,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	client.state_at(block_hash).expect("state should exist").inspect_state(|| {
		assert!(polkadot_test_runtime::System::events().iter().any(|r| matches!(
			r.event,
			polkadot_test_runtime::RuntimeEvent::Xcm(pallet_xcm::Event::ResponseReady(
				q,
				Response::ExecutionResult(None),
			)) if q == query_id,
		)));
		assert_eq!(
			polkadot_test_runtime::Xcm::query(query_id),
			Some(QueryStatus::Ready {
				response: VersionedResponse::V2(Response::ExecutionResult(None)),
				at: 2u32.into()
			}),
		)
	});
}

#[test]
fn query_response_elicits_handler() {
	use pallet_test_notifier::Event::*;
	use polkadot_test_runtime::RuntimeEvent::TestNotifier;

	sp_tracing::try_init_simple();
	let mut client = TestClientBuilder::new()
		.set_execution_strategy(ExecutionStrategy::AlwaysWasm)
		.build();

	let mut block_builder = client.init_polkadot_block_builder();

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::RuntimeCall::TestNotifier(
			pallet_test_notifier::Call::prepare_new_notify_query {},
		),
		sp_keyring::Sr25519Keyring::Alice,
		0,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	let mut query_id = None;
	client.state_at(block_hash).expect("state should exist").inspect_state(|| {
		for r in polkadot_test_runtime::System::events().iter() {
			match r.event {
				TestNotifier(NotifyQueryPrepared(q)) => query_id = Some(q),
				_ => (),
			}
		}
	});
	let query_id = query_id.unwrap();

	let mut block_builder = client.init_polkadot_block_builder();

	let response = Response::ExecutionResult(None);
	let max_weight = 1_000_000;
	let msg = Xcm(vec![QueryResponse { query_id, response, max_weight }]);

	let execute = construct_extrinsic(
		&client,
		polkadot_test_runtime::RuntimeCall::Xcm(pallet_xcm::Call::execute {
			message: Box::new(VersionedXcm::from(msg)),
			max_weight: 1_000_000_000,
		}),
		sp_keyring::Sr25519Keyring::Alice,
		1,
	);

	block_builder.push_polkadot_extrinsic(execute).expect("pushes extrinsic");

	let block = block_builder.build().expect("Finalizes the block").block;
	let block_hash = block.hash();

	futures::executor::block_on(client.import(sp_consensus::BlockOrigin::Own, block))
		.expect("imports the block");

	client.state_at(block_hash).expect("state should exist").inspect_state(|| {
		assert!(polkadot_test_runtime::System::events().iter().any(|r| matches!(
			r.event,
			TestNotifier(ResponseReceived(
				MultiLocation { parents: 0, interior: X1(Junction::AccountId32 { .. }) },
				q,
				Response::ExecutionResult(None),
			)) if q == query_id,
		)));
	});
}
