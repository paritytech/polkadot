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
	let max_weight: frame_support::weights::Weight =
		RocksDbWeight::get().reads_writes(samples.into(), samples.into());
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

	assert!(weight * 50 < BlockWeights::get().max_block);
}
