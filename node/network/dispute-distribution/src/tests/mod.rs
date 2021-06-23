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
//

//! Subsystem unit tests

use assert_matches::assert_matches;
use futures::{Future, channel::mpsc};
use polkadot_primitives::v1::Hash;
use polkadot_subsystem::{FromOverseer, messages::{AllMessages, DisputeDistributionMessage, NetworkBridgeMessage}};
use polkadot_subsystem_testhelpers::{TestSubsystemContextHandle, mock::make_ferdie_keystore, subsystem_test_harness};

use crate::{DisputeDistributionSubsystem, LOG_TARGET};

use self::mock::{ALICE_INDEX, FERDIE_INDEX, make_candidate_receipt, make_dispute_message, MockAuthorityDiscovery};

/// Useful mock providers.
pub mod mock;

#[test]
fn send_dispute_sends_dispute() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>|
		async move {

			handle_subsystem_startup(&mut handle).await;

			let relay_parent = Hash::random();
			let candidate = make_candidate_receipt(relay_parent);
			let message =
				make_dispute_message(candidate, ALICE_INDEX, FERDIE_INDEX,).await;
			handle.send(
				FromOverseer::Communication {
					msg: DisputeDistributionMessage::SendDispute(message)
				}
			).await;
	};
	test_harness(test);
}

/// Initialize subsystem and return request sender needed for sending incoming requests to the
/// subsystem.
async fn handle_subsystem_startup(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>
) -> (mpsc::Sender<sc_network::config::IncomingRequest>, MockAuthorityDiscovery) {
	let (request_tx, request_rx) = mpsc::channel(5);
	handle.send(
		FromOverseer::Communication {
			msg: DisputeDistributionMessage::DisputeSendingReceiver(request_rx),
		}
	).await;

	let authority_discovery = MockAuthorityDiscovery::new();

	assert_matches!(
		handle.recv().await,
		AllMessages::NetworkBridge(
			NetworkBridgeMessage::GetAuthorityDiscoveryService(tx)
		) => {
			tx
				.send(Box::new(authority_discovery.clone()))
				.expect("Receiver shoult still be alive.");
		}
	);
	(request_tx, authority_discovery)
}

/// Launch subsystem and provided test function
///
/// which simulates the overseer.
fn test_harness<TestFn, Fut>(test: TestFn)
where
	TestFn: FnOnce(TestSubsystemContextHandle<DisputeDistributionMessage>) -> Fut,
	Fut: Future<Output = ()>
{
	sp_tracing::try_init_simple();
	let keystore = make_ferdie_keystore();

	let subsystem = DisputeDistributionSubsystem::new(keystore);

	let subsystem = |ctx| async {
		match subsystem.run(ctx).await {
			Ok(()) => {},
			Err(fatal) => {
				tracing::debug!(
					target: LOG_TARGET,
					?fatal,
					"Dispute distribution exited with fatal error."
				);
			}
		}
	};
	subsystem_test_harness(test, subsystem);
}

