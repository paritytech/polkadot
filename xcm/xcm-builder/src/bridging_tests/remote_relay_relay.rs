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

use super::*;

parameter_types! {
	pub UniversalLocation: Junctions = X2(GlobalConsensus(Local::get()), Parachain(1000));
	pub RelayUniversalLocation: Junctions = X1(GlobalConsensus(Local::get()));
	pub RemoteUniversalLocation: Junctions = X1(GlobalConsensus(Remote::get()));
	pub BridgeTable: Vec<(NetworkId, MultiLocation, Option<MultiAsset>)>
		= vec![(Remote::get(), MultiLocation::parent(), None)];
}
type TheBridge =
	TestBridge<BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteUniversalLocation>>;
type RelayExporter = HaulBlobExporter<TheBridge, Remote>;
type LocalInnerRouter =
	UnpaidExecutingRouter<UniversalLocation, RelayUniversalLocation, RelayExporter>;
type LocalBridgeRouter =
	UnpaidRemoteExporter<NetworkExportTable<BridgeTable>, LocalInnerRouter, UniversalLocation>;
type LocalRouter = (LocalInnerRouter, LocalBridgeRouter);

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///            /\                           |
///            ||                           |
///            ||                           |
///            ||                           |
///     Parachain(1000)                     |
/// ```
#[test]
fn sending_to_bridged_chain_works() {
	let msg = Xcm(vec![Trap(1)]);
	assert_eq!(<LocalRouter as SendXcm>::send_xcm((Parent, Parent, Remote::get()), msg), Ok(()));
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

/// ``` nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///            /\                           |                             ||
///            ||                           |                             ||
///            ||                           |                             ||
///            ||                           |                             \/
///     Parachain(1000)                     |                       Parachain(1000)
/// ```
#[test]
fn sending_to_parachain_of_bridged_chain_works() {
	let dest = (Parent, Parent, Remote::get(), Parachain(1000));
	assert_eq!(LocalRouter::send_xcm(dest, Xcm(vec![Trap(1)])), Ok(()));
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(
		Parachain(1000).into(),
		Xcm(vec![
			UniversalOrigin(Local::get().into()),
			DescendOrigin(Parachain(1000).into()),
			Trap(1),
		]),
	)];
	assert_eq!(take_received_remote_messages(), expected);
}
