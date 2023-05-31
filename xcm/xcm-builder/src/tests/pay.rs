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

use super::*;
use frame_support::{assert_ok, traits::tokens::Pay};
use sp_runtime::traits::Convert;

type BlockNumber = u64;
type AccountId = u64;

parameter_types! {
	pub Interior: InteriorMultiLocation = AccountIndex64 { index: 3u64, network: None }.into();
	pub Timeout: BlockNumber = 5; // 5 blocks
	pub Beneficiary: AccountId = 5;
}

#[derive(Encode, Decode, Clone, Copy, PartialEq, Eq)]
pub struct AssetKind {
	destination: xcm::latest::MultiLocation,
	asset_id: xcm::latest::AssetId,
}

pub struct LocatableAssetKindConverter;
impl sp_runtime::traits::Convert<AssetKind, LocatableAssetId> for LocatableAssetKindConverter {
	fn convert(value: AssetKind) -> LocatableAssetId {
		LocatableAssetId { asset_id: value.asset_id, location: value.destination }
	}
}

pub struct AliasesIntoAccountIndex64;
impl<'a> sp_runtime::traits::Convert<&'a AccountId, MultiLocation> for AliasesIntoAccountIndex64 {
	fn convert(who: &AccountId) -> MultiLocation {
		AccountIndex64 { network: None, index: who.clone().into() }.into()
	}
}

#[test]
fn pay_over_xcm_works() {
	AllowExplicitUnpaidFrom::set(vec![X1(Parachain(1)).into()]);
	add_asset(AliasesIntoAccountIndex64::convert(&3u64), (Here, 1000u128));
	let who = 5u64;
	let asset_kind =
		AssetKind { destination: (Parent, Parachain(2)).into(), asset_id: Here.into() };
	let amount = 1000u128;
	assert_ok!(PayOverXcm::<
		Interior,
		TestMessageSender,
		TestQueryHandler<TestConfig, BlockNumber>,
		Timeout,
		AccountId,
		AssetKind,
		LocatableAssetKindConverter,
		AliasesIntoAccountIndex64,
	>::pay(&who, asset_kind, amount));
	dbg!(&sent_xcm());
}
