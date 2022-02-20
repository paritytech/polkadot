// Copyright 2020 Parity Technologies query_id: (), max_response_weight: ()  query_id: (), max_response_weight: ()  (UK) Ltd.
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

use super::*;

#[test]
fn universal_origin_should_work() {
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into(), X1(Parachain(2)).into()]);
	clear_universal_aliases();
	// Parachain 1 may represent Kusama to us
	add_universal_alias(Parachain(1), Kusama);
	// Parachain 2 may represent Polkadot to us
	add_universal_alias(Parachain(2), Polkadot);

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(2),
		Xcm(vec![
			UniversalOrigin(GlobalConsensus(Kusama)),
			TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::InvalidLocation));

	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			UniversalOrigin(GlobalConsensus(Kusama)),
			TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::NotWithdrawable));

	add_asset(4000, (Parent, 100));
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![
			UniversalOrigin(GlobalConsensus(Kusama)),
			TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
		]),
		50,
	);
	assert_eq!(r, Outcome::Complete(20));
	assert_eq!(assets(4000), vec![]);
}

#[test]
fn export_message_should_work() {
	// Bridge chain (assumed to be Relay) lets Parachain #1 have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Local parachain #1 issues a transfer asset on Polkadot Relay-chain, transfering 100 Planck to
	// Polkadot parachain #2.
	let message =
		Xcm(vec![TransferAsset { assets: (Here, 100).into(), beneficiary: Parachain(2).into() }]);
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		Parachain(1),
		Xcm(vec![ExportMessage { network: Polkadot, destination: Here, xcm: message.clone() }]),
		50,
	);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(exported_xcm(), vec![(Polkadot, 403611790, Here, message)]);
}
