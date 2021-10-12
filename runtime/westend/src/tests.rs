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

#[test]
fn remove_keys_weight_is_sensible() {
	use runtime_common::crowdloan::WeightInfo;
	let max_weight = <Runtime as crowdloan::Config>::WeightInfo::refund(RemoveKeysLimit::get());
	// Max remove keys limit should be no more than half the total block weight.
	assert!(max_weight * 2 < BlockWeights::get().max_block);
}

#[test]
fn sample_size_is_sensible() {
	use runtime_common::auctions::WeightInfo;
	// Need to clean up all samples at the end of an auction.
	let samples: BlockNumber = EndingPeriod::get() / SampleLength::get();
	let max_weight: Weight = RocksDbWeight::get().reads_writes(samples.into(), samples.into());
	// Max sample cleanup should be no more than half the total block weight.
	assert!(max_weight * 2 < BlockWeights::get().max_block);
	assert!(
		<Runtime as auctions::Config>::WeightInfo::on_initialize() * 2 <
			BlockWeights::get().max_block
	);
}

#[test]
fn call_size() {
	assert!(
		core::mem::size_of::<Call>() <= 230,
		"size of Call is more than 230 bytes: some calls have too big arguments, use Box to reduce \
		the size of Call.
		If the limit is too strong, maybe consider increase the limit to 300.",
	);
}

#[test]
fn xcm_weight() {
	use frame_support::dispatch::GetDispatchInfo;
	let weight = pallet_xcm::Call::<Runtime>::teleport_assets {
		dest: Box::new(xcm::VersionedMultiLocation::V1(MultiLocation::parent())),
		beneficiary: Box::new(xcm::VersionedMultiLocation::V1(MultiLocation::parent())),
		assets: Box::new((
			Concrete(MultiLocation::parent()),
			Fungible(200_000)
		).into()),
		fee_asset_item: 0
	}.get_dispatch_info().weight;

	println!("{:?} max: {:?}", weight, u64::MAX);
	assert!(false);
}

#[test]
fn xcm_weight_2() {
	use sp_std::convert::TryInto;
	use xcm_executor::traits::WeightBounds;
	let assets: Box<xcm::VersionedMultiAssets> = Box::new((
		Concrete(MultiLocation::parent()),
		Fungible(200_000)
	).into());
	let dest: Box<xcm::VersionedMultiLocation> = Box::new(
		xcm::VersionedMultiLocation::V1(MultiLocation::parent())
	);
	let maybe_assets: Result<MultiAssets, ()> = (*assets.clone()).try_into();
	let maybe_dest: Result<MultiLocation, ()> = (*dest.clone()).try_into();
	let final_weight = match (maybe_assets, maybe_dest) {
		(Ok(assets), Ok(dest)) => {
			use sp_std::vec;
			let mut message = Xcm(vec![
				WithdrawAsset(assets),
				InitiateTeleport { assets: Wild(All), dest, xcm: Xcm(vec![]) },
			]);
			let result = <XcmConfig as xcm_executor::Config>::Weigher::weight(&mut message);
			println!("result: {:?}", result);
			result.map_or(Weight::max_value(), |w| 100_000_000 + w)
		},
		_ => Weight::max_value(),
	};

	println!("{:?} max: {:?}", final_weight, u64::MAX);
	assert!(false);
}
