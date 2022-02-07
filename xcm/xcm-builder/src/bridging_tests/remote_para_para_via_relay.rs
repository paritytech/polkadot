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

//! This test is when we're sending an XCM from a relay-chain whose child parachain hosts a
//! bridge to a parachain from another global consensus. The destination of the XCM is within
//! the global consensus of the remote side of the bridge.

use super::*;

parameter_types! {
	pub UniversalLocation: Junctions = X1(GlobalConsensus(Local::get()));
	pub ParaBridgeUniversalLocation: Junctions = X2(GlobalConsensus(Local::get()), Parachain(1));
	pub RemoteParaBridgeUniversalLocation: Junctions = X2(GlobalConsensus(Remote::get()), Parachain(1));
	pub BridgeTable: Vec<(NetworkId, MultiLocation, Option<MultiAsset>)>
		= vec![(Remote::get(), Parachain(1).into(), None)];
}
type TheBridge =
	TestBridge<BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteParaBridgeUniversalLocation>>;
type RelayExporter = HaulBlobExporter<TheBridge, Remote, ()>;
type LocalInnerRouter =
	UnpaidExecutingRouter<UniversalLocation, ParaBridgeUniversalLocation, RelayExporter>;
type LocalBridgingRouter =
	UnpaidRemoteExporter<NetworkExportTable<BridgeTable>, LocalInnerRouter, UniversalLocation>;
type LocalRouter = (LocalInnerRouter, LocalBridgingRouter);

/// ```nocompile
///  local                                    |                                      remote
///                                           |
///     GlobalConsensus(Local::get())         |         GlobalConsensus(Remote::get())
///                              ||           |
///                              ||           |
///                              ||           |
///                              \/           |
///                            Parachain(1)  ===>  Parachain(1)
/// ```
#[test]
fn sending_to_bridged_chain_works() {
	let msg = Xcm(vec![Trap(1)]);
	assert_eq!(
		send_xcm::<LocalRouter>((Parent, Remote::get(), Parachain(1)).into(), msg),
		Ok(MultiAssets::new())
	);
	assert_eq!(TheBridge::service(), 1);
	assert_eq!(
		take_received_remote_messages(),
		vec![(Here.into(), Xcm(vec![UniversalOrigin(Local::get().into()), Trap(1)]))]
	);
}

/// ```nocompile
///  local                                    |                                      remote
///                                           |
///     GlobalConsensus(Local::get())         |         GlobalConsensus(Remote::get())
///                              ||           |
///                              ||           |
///                              ||           |
///                              \/           |
///                            Parachain(1)  ===>  Parachain(1)  ===>  Parachain(1000)
/// ```
#[test]
fn sending_to_sibling_of_bridged_chain_works() {
	let dest = (Parent, Remote::get(), Parachain(1000)).into();
	assert_eq!(send_xcm::<LocalRouter>(dest, Xcm(vec![Trap(1)])), Ok(MultiAssets::new()));
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(
		(Parent, Parachain(1000)).into(),
		Xcm(vec![UniversalOrigin(Local::get().into()), Trap(1)]),
	)];
	assert_eq!(take_received_remote_messages(), expected);
}

/// ```nocompile
///  local                                    |                                      remote
///                                           |
///     GlobalConsensus(Local::get())         |         GlobalConsensus(Remote::get())
///                              ||           |            /\
///                              ||           |            ||
///                              ||           |            ||
///                              \/           |            ||
///                            Parachain(1)  ===>  Parachain(1)
/// ```
#[test]
fn sending_to_relay_of_bridged_chain_works() {
	let dest = (Parent, Remote::get()).into();
	assert_eq!(send_xcm::<LocalRouter>(dest, Xcm(vec![Trap(1)])), Ok(MultiAssets::new()));
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(Parent.into(), Xcm(vec![UniversalOrigin(Local::get().into()), Trap(1)]))];
	assert_eq!(take_received_remote_messages(), expected);
}
