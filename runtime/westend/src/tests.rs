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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! Tests for the Westend Runtime Configuration

use crate::*;
use xcm::latest::{AssetId::*, Fungibility::*, MultiLocation};
use xcm_runtime_api::runtime_decl_for_XcmApi::XcmApi;

#[test]
fn remove_keys_weight_is_sensible() {
	use runtime_common::crowdloan::WeightInfo;
	let max_weight = <Runtime as crowdloan::Config>::WeightInfo::refund(RemoveKeysLimit::get());
	// Max remove keys limit should be no more than half the total block weight.
	assert!((max_weight * 2).all_lt(BlockWeights::get().max_block));
}

#[test]
fn sample_size_is_sensible() {
	use runtime_common::auctions::WeightInfo;
	// Need to clean up all samples at the end of an auction.
	let samples: BlockNumber = EndingPeriod::get() / SampleLength::get();
	let max_weight: frame_support::weights::Weight =
		RocksDbWeight::get().reads_writes(samples.into(), samples.into());
	// Max sample cleanup should be no more than half the total block weight.
	assert!((max_weight * 2).all_lt(BlockWeights::get().max_block));
	assert!((<Runtime as auctions::Config>::WeightInfo::on_initialize() * 2)
		.all_lt(BlockWeights::get().max_block));
}

#[test]
fn call_size() {
	assert!(
		core::mem::size_of::<RuntimeCall>() <= 230,
		"size of RuntimeCall is more than 230 bytes: some calls have too big arguments, use Box to reduce \
		the size of RuntimeCall.
		If the limit is too strong, maybe consider increase the limit to 300.",
	);
}

#[test]
fn sanity_check_teleport_assets_weight() {
	// This test sanity checks that at least 50 teleports can exist in a block.
	// Usually when XCM runs into an issue, it will return a weight of `Weight::MAX`,
	// so this test will certainly ensure that this problem does not occur.
	use frame_support::dispatch::GetDispatchInfo;
	let weight = pallet_xcm::Call::<Runtime>::teleport_assets {
		dest: Box::new(xcm::VersionedMultiLocation::V1(MultiLocation::here())),
		beneficiary: Box::new(xcm::VersionedMultiLocation::V1(MultiLocation::here())),
		assets: Box::new((Concrete(MultiLocation::here()), Fungible(200_000)).into()),
		fee_asset_item: 0,
	}
	.get_dispatch_info()
	.weight;

	assert!((weight * 50).all_lt(BlockWeights::get().max_block));
}

#[test]
fn xcm_runtime_api_weight() {
	use xcm::v2::XcmWeightInfo;
	let message = xcm::VersionedXcm::<RuntimeCall>::V2(xcm::latest::Xcm(vec![
		xcm::latest::prelude::ClearOrigin,
	]));
	assert_eq!(
		Runtime::weigh_message(message),
		Ok(weights::xcm::WestendXcmWeight::<RuntimeCall>::clear_origin().into())
	);
}

#[test]
fn xcm_runtime_location_convert() {
	use xcm::latest::prelude::*;
	use xcm_executor::traits::Convert;
	let para_location = xcm::latest::MultiLocation::new(0, X1(Parachain(1000u32)));
	assert_eq!(
		Runtime::convert_location(para_location.clone().into()),
		Ok(xcm_builder::ChildParachainConvertsVia::<ParaId, AccountId>::convert_ref(para_location)
			.unwrap())
	);
	let local_location = xcm::latest::MultiLocation::new(
		0,
		X1(AccountId32 { network: NetworkId::Any, id: [1u8; 32] }),
	);
	assert_eq!(
		Runtime::convert_location(local_location.clone().into()),
		Ok(xcm_builder::AccountId32Aliases::<xcm_config::WestendNetwork, AccountId>::convert_ref(
			local_location
		)
		.unwrap())
	);
}

#[test]
fn xcm_asset() {
	use frame_support::weights::WeightToFee as WeightToFeeT;
	use xcm::latest::prelude::*;
	let asset_location: MultiLocation = Here.into();
	let t = frame_system::GenesisConfig::default()
		.build_storage::<Runtime>()
		.expect("Frame system builds valid default genesis config");
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| {
		let result = Runtime::calculate_concrete_asset_fee(
			asset_location.into(),
			ExtrinsicBaseWeight::get().ref_time().into(),
		);
		assert_eq!(result, Ok(WeightToFee::weight_to_fee(&ExtrinsicBaseWeight::get())));
	});
}
