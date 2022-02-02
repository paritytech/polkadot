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

//! This test is when we're sending an XCM from a parachain whose relay-chain hosts a bridge to
//! another relay-chain. The destination of the XCM is within the global consensus of the
//! remote side of the bridge.

use xcm_executor::XcmExecutor;
use crate::mock::*;
use super::*;

parameter_types! {
	pub Local: NetworkId = ByUri(b"local".to_vec());
	pub UniversalLocation: Junctions = X2(GlobalConsensus(Local::get()), Parachain(1000));
	pub RelayUniversalLocation: Junctions = X1(GlobalConsensus(Local::get()));
	pub Remote: NetworkId = ByUri(b"remote".to_vec());
	pub RemoteUniversalLocation: Junctions = X1(GlobalConsensus(Remote::get()));
	pub BridgeTable: Vec<(NetworkId, MultiLocation, Option<MultiAsset>)>
		= vec![(Remote::get(), MultiLocation::parent(), None)];
}
type TheBridge =
	TestBridge<BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteUniversalLocation>>;
type RelayExporter = HaulBlobExporter<TheBridge, Remote>;
type TestExecutor = XcmExecutor<TestConfig>;

/// This is a dummy router which accepts messages destined for our `Parent` and executes
/// them in a context simulated to be like that of our `Parent`.
struct LocalInnerRouter;
impl SendXcm for LocalInnerRouter {
	fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
		let destination = destination.into();
		if destination == Parent.into() {
			// We now pretend that the message was delivered from the local chain
			// Parachain(1000) to the relay-chain and is getting executed there. We need to
			// configure the TestExecutor appropriately:
			ExecutorUniversalLocation::set(RelayUniversalLocation::get());
			let origin = UniversalLocation::get().relative_to(&RelayUniversalLocation::get());
			AllowUnpaidFrom::set(vec![origin.clone()]);
			set_exporter_override(RelayExporter::export_xcm);
			// The we execute it:
			let outcome = TestExecutor::execute_xcm(origin, message.into(), 1_000_000);
			assert_matches!(outcome, Outcome::Complete(_));
			return Ok(())
		}
		Err(SendError::CannotReachDestination(destination, message))
	}
}
type LocalBridgingRouter = UnpaidRemoteExporter<
	NetworkExportTable<BridgeTable>,
	LocalInnerRouter,
	UniversalLocation,
>;
type LocalRouter = (
	LocalInnerRouter,
	LocalBridgingRouter,
);

///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///            /\                           |
///            ||                           |
///            ||                           |
///            ||                           |
///     Parachain(1000)                     |
#[test]
fn sending_to_bridged_chain_works() {
	let msg = Xcm(vec![Trap(1)]);
	assert_eq!(<LocalRouter as SendXcm>::send_xcm((Parent, Parent, ByUri(b"remote".to_vec())), msg), Ok(()));
	assert_eq!(TheBridge::service(), 1);
	assert_eq!(
		take_received_remote_messages(),
		vec![(
			Here.into(),
			Xcm(vec![
				UniversalOrigin(Local::get().into()),
				DescendOrigin(Parachain(1000).into()),
				Trap(1)
			])
		)]
	);
}

///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///            /\                           |                             ||
///            ||                           |                             ||
///            ||                           |                             ||
///            ||                           |                             \/
///     Parachain(1000)                     |                       Parachain(1000)
#[test]
fn sending_to_parachain_of_bridged_chain_works() {
	let dest = (Parent, Parent, ByUri(b"remote".to_vec()), Parachain(1000));
	assert_eq!(LocalRouter::send_xcm(dest, Xcm(vec![Trap(1)])), Ok(()));
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(
		Parachain(1000).into(),
		Xcm(vec![
			UniversalOrigin(Local::get().into()),
			DescendOrigin(Parachain(1000).into()),
			Trap(1)
		])
	)];
	assert_eq!(take_received_remote_messages(), expected);
}
