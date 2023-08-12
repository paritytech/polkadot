// Copyright (C) Parity Technologies (UK) Ltd.
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

//! This test is when we're sending an XCM from a relay-chain which hosts a bridge to another
//! relay-chain. The destination of the XCM is within the global consensus of the
//! remote side of the bridge.

use super::*;

parameter_types! {
	pub UniversalLocation: Junctions = X1(GlobalConsensus(Local::get()));
	pub RemoteUniversalLocation: Junctions = X1(GlobalConsensus(Remote::get()));
}
type TheBridge =
	TestBridge<BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteUniversalLocation, ()>>;
type Router =
	TestTopic<UnpaidLocalExporter<HaulBlobExporter<TheBridge, Remote, Price>, UniversalLocation>>;

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///                                         |
/// ```
#[test]
fn sending_to_bridged_chain_works() {
	maybe_with_topic(|| {
		let msg = Xcm(vec![Trap(1)]);
		assert_eq!(
			send_xcm::<Router>((Parent, Remote::get()).into(), msg).unwrap().1,
			(Here, 100).into()
		);
		assert_eq!(TheBridge::service(), 1);
		let expected = vec![(
			Here.into(),
			xcm_with_topic([0; 32], vec![UniversalOrigin(Local::get().into()), Trap(1)]),
		)];
		assert_eq!(take_received_remote_messages(), expected);
		assert_eq!(RoutingLog::take(), vec![]);
	});
}

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///                                         |                             ||
///                                         |                             ||
///                                         |                             ||
///                                         |                             \/
///                                         |                       Parachain(1000)
/// ```
#[test]
fn sending_to_parachain_of_bridged_chain_works() {
	maybe_with_topic(|| {
		let msg = Xcm(vec![Trap(1)]);
		let dest = (Parent, Remote::get(), Parachain(1000)).into();
		assert_eq!(send_xcm::<Router>(dest, msg).unwrap().1, (Here, 100).into());
		assert_eq!(TheBridge::service(), 1);
		let expected = vec![(
			Parachain(1000).into(),
			xcm_with_topic([0; 32], vec![UniversalOrigin(Local::get().into()), Trap(1)]),
		)];
		assert_eq!(take_received_remote_messages(), expected);
		assert_eq!(RoutingLog::take(), vec![]);
	});
}
