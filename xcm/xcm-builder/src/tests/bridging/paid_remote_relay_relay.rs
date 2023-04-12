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

//! This test is when we're sending an XCM from a parachain whose relay-chain hosts a bridge to
//! another relay-chain. The destination of the XCM is within the global consensus of the
//! remote side of the bridge.
//!
//! The Relay-chain here requires payment by the parachain for use of the bridge. This is expressed
//! under the standard XCM weight and the weight pricing.

use super::*;

parameter_types! {
	pub UniversalLocation: Junctions = X2(GlobalConsensus(Local::get()), Parachain(100));
	pub RelayUniversalLocation: Junctions = X1(GlobalConsensus(Local::get()));
	pub RemoteUniversalLocation: Junctions = X1(GlobalConsensus(Remote::get()));
	pub static BridgeTable: Vec<(NetworkId, MultiLocation, Option<MultiAsset>)>
		= vec![(Remote::get(), MultiLocation::parent(), Some((Parent, 200u128).into()))];
	// ^^^ 100 to use the bridge (export) and 100 for the remote execution weight (5 instructions
	//     x (10 + 10) weight each).
}
type TheBridge =
	TestBridge<BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteUniversalLocation>>;
type RelayExporter = HaulBlobExporter<TheBridge, Remote, Price>;
type LocalInnerRouter = ExecutingRouter<UniversalLocation, RelayUniversalLocation, RelayExporter>;
type LocalBridgeRouter = SovereignPaidRemoteExporter<
	NetworkExportTable<BridgeTable>,
	LocalInnerRouter,
	UniversalLocation,
>;
type LocalRouter = (LocalInnerRouter, LocalBridgeRouter);

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///            /\                           |
///            ||                           |
///            ||                           |
///            ||                           |
///     Parachain(100)                      |
/// ```
#[test]
fn sending_to_bridged_chain_works() {
	let dest: MultiLocation = (Parent, Parent, Remote::get()).into();
	// Routing won't work if we don't have enough funds.
	assert_eq!(
		send_xcm::<LocalRouter>(dest.clone(), Xcm(vec![Trap(1)])),
		Err(SendError::Transport("Error executing")),
	);

	// Initialize the local relay so that our parachain has funds to pay for export.
	add_asset(Parachain(100), (Here, 1000u128));

	let msg = Xcm(vec![Trap(1)]);
	assert_eq!(send_xcm::<LocalRouter>(dest, msg).unwrap().1, (Parent, 200u128).into());
	assert_eq!(TheBridge::service(), 1);
	assert_eq!(
		take_received_remote_messages(),
		vec![(
			Here.into(),
			Xcm(vec![
				UniversalOrigin(Local::get().into()),
				DescendOrigin(Parachain(100).into()),
				Trap(1),
			])
		)]
	);

	// The export cost 50 ref time and 50 proof size weight units (and thus 100 units of balance).
	assert_eq!(asset_list(Parachain(100)), vec![(Here, 800u128).into()]);
}

/// ```nocompile
///  local                                  |                                      remote
///                                         |
///     GlobalConsensus(Local::get())   ========>    GlobalConsensus(Remote::get())
///            /\                           |                             ||
///            ||                           |                             ||
///            ||                           |                             ||
///            ||                           |                             \/
///     Parachain(100)                      |                       Parachain(100)
/// ```
#[test]
fn sending_to_parachain_of_bridged_chain_works() {
	let dest: MultiLocation = (Parent, Parent, Remote::get(), Parachain(100)).into();
	// Routing won't work if we don't have enough funds.
	assert_eq!(
		send_xcm::<LocalRouter>(dest.clone(), Xcm(vec![Trap(1)])),
		Err(SendError::Transport("Error executing")),
	);

	// Initialize the local relay so that our parachain has funds to pay for export.
	add_asset(Parachain(100), (Here, 1000u128));

	let msg = Xcm(vec![Trap(1)]);
	assert_eq!(send_xcm::<LocalRouter>(dest, msg).unwrap().1, (Parent, 200u128).into());
	assert_eq!(TheBridge::service(), 1);
	let expected = vec![(
		Parachain(100).into(),
		Xcm(vec![
			UniversalOrigin(Local::get().into()),
			DescendOrigin(Parachain(100).into()),
			Trap(1),
		]),
	)];
	assert_eq!(take_received_remote_messages(), expected);

	// The export cost 50 ref time and 50 proof size weight units (and thus 100 units of balance).
	assert_eq!(asset_list(Parachain(100)), vec![(Here, 800u128).into()]);
}
