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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

use crate::*;
use alloc::vec;

#[test]
fn encode_decode_versioned_asset_id_v3() {
	let asset_id = VersionedAssetId::V3(v3::AssetId::Abstract([1; 32]));
	let encoded = asset_id.encode();

	assert_eq!(
		encoded,
		hex_literal::hex!("03010101010101010101010101010101010101010101010101010101010101010101"),
		"encode format changed"
	);
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedAssetId::decode(&mut &encoded[..]).unwrap();
	assert_eq!(asset_id, decoded);
}

#[test]
fn encode_decode_versioned_response_v2() {
	let response = VersionedResponse::V2(v2::Response::Null);
	let encoded = response.encode();

	assert_eq!(encoded, hex_literal::hex!("0200"), "encode format changed");
	assert_eq!(encoded[0], 2, "bad version number");

	let decoded = VersionedResponse::decode(&mut &encoded[..]).unwrap();
	assert_eq!(response, decoded);
}

#[test]
fn encode_decode_versioned_response_v3() {
	let response = VersionedResponse::V3(v3::Response::Null);
	let encoded = response.encode();

	assert_eq!(encoded, hex_literal::hex!("0300"), "encode format changed");
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedResponse::decode(&mut &encoded[..]).unwrap();
	assert_eq!(response, decoded);
}

#[test]
fn encode_decode_versioned_multi_location_v2() {
	let location = VersionedMultiLocation::V2(v2::MultiLocation::new(0, v2::Junctions::Here));
	let encoded = location.encode();

	assert_eq!(encoded, hex_literal::hex!("010000"), "encode format changed");
	assert_eq!(encoded[0], 1, "bad version number"); // this is introduced in v1

	let decoded = VersionedMultiLocation::decode(&mut &encoded[..]).unwrap();
	assert_eq!(location, decoded);
}

#[test]
fn encode_decode_versioned_multi_location_v3() {
	let location = VersionedMultiLocation::V3(v3::MultiLocation::new(0, v3::Junctions::Here));
	let encoded = location.encode();

	assert_eq!(encoded, hex_literal::hex!("030000"), "encode format changed");
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedMultiLocation::decode(&mut &encoded[..]).unwrap();
	assert_eq!(location, decoded);
}

#[test]
fn encode_decode_versioned_interior_multi_location_v2() {
	let location = VersionedInteriorMultiLocation::V2(v2::InteriorMultiLocation::Here);
	let encoded = location.encode();

	assert_eq!(encoded, hex_literal::hex!("0200"), "encode format changed");
	assert_eq!(encoded[0], 2, "bad version number");

	let decoded = VersionedInteriorMultiLocation::decode(&mut &encoded[..]).unwrap();
	assert_eq!(location, decoded);
}

#[test]
fn encode_decode_versioned_interior_multi_location_v3() {
	let location = VersionedInteriorMultiLocation::V3(v3::InteriorMultiLocation::Here);
	let encoded = location.encode();

	assert_eq!(encoded, hex_literal::hex!("0300"), "encode format changed");
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedInteriorMultiLocation::decode(&mut &encoded[..]).unwrap();
	assert_eq!(location, decoded);
}

#[test]
fn encode_decode_versioned_multi_asset_v2() {
	let asset = VersionedMultiAsset::V2(v2::MultiAsset::from(((0, v2::Junctions::Here), 1)));
	let encoded = asset.encode();

	assert_eq!(encoded, hex_literal::hex!("010000000004"), "encode format changed");
	assert_eq!(encoded[0], 1, "bad version number");

	let decoded = VersionedMultiAsset::decode(&mut &encoded[..]).unwrap();
	assert_eq!(asset, decoded);
}

#[test]
fn encode_decode_versioned_multi_asset_v3() {
	let asset = VersionedMultiAsset::V3(v3::MultiAsset::from((v3::MultiLocation::default(), 1)));
	let encoded = asset.encode();

	assert_eq!(encoded, hex_literal::hex!("030000000004"), "encode format changed");
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedMultiAsset::decode(&mut &encoded[..]).unwrap();
	assert_eq!(asset, decoded);
}

#[test]
fn encode_decode_versioned_multi_assets_v2() {
	let assets = VersionedMultiAssets::V2(v2::MultiAssets::from(vec![v2::MultiAsset::from((
		(0, v2::Junctions::Here),
		1,
	))]));
	let encoded = assets.encode();

	assert_eq!(encoded, hex_literal::hex!("01040000000004"), "encode format changed");
	assert_eq!(encoded[0], 1, "bad version number");

	let decoded = VersionedMultiAssets::decode(&mut &encoded[..]).unwrap();
	assert_eq!(assets, decoded);
}

#[test]
fn encode_decode_versioned_multi_assets_v3() {
	let assets = VersionedMultiAssets::V3(v3::MultiAssets::from(vec![
		(v3::MultiAsset::from((v3::MultiLocation::default(), 1))),
	]));
	let encoded = assets.encode();

	assert_eq!(encoded, hex_literal::hex!("03040000000004"), "encode format changed");
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedMultiAssets::decode(&mut &encoded[..]).unwrap();
	assert_eq!(assets, decoded);
}

#[test]
fn encode_decode_versioned_xcm_v2() {
	let xcm = VersionedXcm::V2(v2::Xcm::<()>::new());
	let encoded = xcm.encode();

	assert_eq!(encoded, hex_literal::hex!("0200"), "encode format changed");
	assert_eq!(encoded[0], 2, "bad version number");

	let decoded = VersionedXcm::decode(&mut &encoded[..]).unwrap();
	assert_eq!(xcm, decoded);
}

#[test]
fn encode_decode_versioned_xcm_v3() {
	let xcm = VersionedXcm::V3(v3::Xcm::<()>::new());
	let encoded = xcm.encode();

	assert_eq!(encoded, hex_literal::hex!("0300"), "encode format changed");
	assert_eq!(encoded[0], 3, "bad version number");

	let decoded = VersionedXcm::decode(&mut &encoded[..]).unwrap();
	assert_eq!(xcm, decoded);
}
