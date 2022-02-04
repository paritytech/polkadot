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

//! This test is when we're sending an XCM from a parachain which hosts a bridge to another
//! network's bridge parachain. The destination of the XCM is within the global consensus of the
//! remote side of the bridge.

use super::*;

parameter_types! {
	pub UniversalLocation: Junctions = X2(GlobalConsensus(Local::get()), Parachain(1));
	pub RemoteUniversalLocation: Junctions = X2(GlobalConsensus(Remote::get()), Parachain(1));
}
type TheBridge =
	TestBridge<BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteUniversalLocation>>;
type Router = LocalUnpaidExporter<HaulBlobExporter<TheBridge, Remote>, UniversalLocation>;

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())       |        GlobalConsensus(Remote::get())
///                                         |
///                                         |
///                                         |
///                                         |
///                          Parachain(1)  ===>  Parachain(1)
/// ```
#[test]
fn sending_to_bridged_chain_works() {
	let msg = Xcm(vec![Trap(1)]);
	assert_eq!(Router::send_xcm((Parent, Parent, Remote::get(), Parachain(1)), msg), Ok(()));
	assert_eq!(TheBridge::service(), 1);
	assert_eq!(
		take_received_remote_messages(),
		vec![(
			Here.into(),
			Xcm(vec![
				UniversalOrigin(Local::get().into()),
				DescendOrigin(Parachain(1).into()),
				Trap(1),
			])
		)]
	);
}

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())       |        GlobalConsensus(Remote::get())
///                                         |
///                                         |
///                                         |
///                                         |
///                          Parachain(1)  ===>  Parachain(1)  ==>  Parachain(1000)
/// ```
#[test]
fn sending_to_parachain_of_bridged_chain_works() {
	let dest = (Parent, Parent, Remote::get(), Parachain(1000));
	assert_eq!(Router::send_xcm(dest, Xcm(vec![Trap(1)])), Ok(()));
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(
		(Parent, Parachain(1000)).into(),
		Xcm(vec![
			UniversalOrigin(Local::get().into()),
			DescendOrigin(Parachain(1).into()),
			Trap(1),
		]),
	)];
	assert_eq!(take_received_remote_messages(), expected);
}

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())       |        GlobalConsensus(Remote::get())
///                                         |           /\
///                                         |           ||
///                                         |           ||
///                                         |           ||
///                          Parachain(1)  ===>  Parachain(1)
/// ```
#[test]
fn sending_to_relay_chain_of_bridged_chain_works() {
	let dest = (Parent, Parent, Remote::get());
	assert_eq!(Router::send_xcm(dest, Xcm(vec![Trap(1)])), Ok(()));
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(
		Parent.into(),
		Xcm(vec![
			UniversalOrigin(Local::get().into()),
			DescendOrigin(Parachain(1).into()),
			Trap(1),
		]),
	)];
	assert_eq!(take_received_remote_messages(), expected);
}
