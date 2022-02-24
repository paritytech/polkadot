// Copyright 2022 Parity Technologies (UK) Ltd.
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

	let message = Xcm(vec![
		UniversalOrigin(GlobalConsensus(Kusama)),
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);
	let hash = VersionedXcm::from(message.clone()).using_encoded(sp_io::hashing::blake2_256);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parachain(2), message, hash, 50);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::InvalidLocation));

	let message = Xcm(vec![
		UniversalOrigin(GlobalConsensus(Kusama)),
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);
	let hash = VersionedXcm::from(message.clone()).using_encoded(sp_io::hashing::blake2_256);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parachain(1), message, hash, 50);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::NotWithdrawable));

	add_asset((Ancestor(2), GlobalConsensus(Kusama)), (Parent, 100));
	let message = Xcm(vec![
		UniversalOrigin(GlobalConsensus(Kusama)),
		TransferAsset { assets: (Parent, 100).into(), beneficiary: Here.into() },
	]);
	let hash = VersionedXcm::from(message.clone()).using_encoded(sp_io::hashing::blake2_256);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parachain(1), message, hash, 50);
	assert_eq!(r, Outcome::Complete(20));
	assert_eq!(asset_list((Ancestor(2), GlobalConsensus(Kusama))), vec![]);
}

#[test]
fn export_message_should_work() {
	// Bridge chain (assumed to be Relay) lets Parachain #1 have message execution for free.
	AllowUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	// Local parachain #1 issues a transfer asset on Polkadot Relay-chain, transfering 100 Planck to
	// Polkadot parachain #2.
	let message =
		Xcm(vec![TransferAsset { assets: (Here, 100).into(), beneficiary: Parachain(2).into() }]);
	let exported_msg =
		Xcm(vec![ExportMessage { network: Polkadot, destination: Here, xcm: message.clone() }]);
	let hash = VersionedXcm::from(exported_msg.clone()).using_encoded(sp_io::hashing::blake2_256);
	let expected_hash = VersionedXcm::from(message.clone()).using_encoded(blake2_256);
	let r = XcmExecutor::<TestConfig>::execute_xcm(Parachain(1), exported_msg, hash, 50);
	assert_eq!(r, Outcome::Complete(10));
	assert_eq!(exported_xcm(), vec![(Polkadot, 403611790, Here, message, expected_hash)]);
}
