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

// Shared test utilities and implementations for the XCM Builder.

use frame_support::{
	parameter_types,
	traits::{Contains, CrateVersion, PalletInfoData, PalletsInfoAccess},
};
use sp_std::vec::Vec;
pub use xcm::latest::{prelude::*, Weight};
use xcm_executor::traits::{ClaimAssets, DropAssets, VersionChangeNotifier};
pub use xcm_executor::{
	traits::{
		AssetExchange, AssetLock, ConvertOrigin, Enact, LockError, OnResponse, TransactAsset,
	},
	Assets, Config,
};

parameter_types! {
	pub static SubscriptionRequests: Vec<(MultiLocation, Option<(QueryId, Weight)>)> = vec![];
	pub static MaxAssetsIntoHolding: u32 = 4;
}

pub struct TestSubscriptionService;

impl VersionChangeNotifier for TestSubscriptionService {
	fn start(
		location: &MultiLocation,
		query_id: QueryId,
		max_weight: Weight,
		_context: &XcmContext,
	) -> XcmResult {
		let mut r = SubscriptionRequests::get();
		r.push((*location, Some((query_id, max_weight))));
		SubscriptionRequests::set(r);
		Ok(())
	}
	fn stop(location: &MultiLocation, _context: &XcmContext) -> XcmResult {
		let mut r = SubscriptionRequests::get();
		r.retain(|(l, _q)| l != location);
		r.push((*location, None));
		SubscriptionRequests::set(r);
		Ok(())
	}
	fn is_subscribed(location: &MultiLocation) -> bool {
		let r = SubscriptionRequests::get();
		r.iter().any(|(l, q)| l == location && q.is_some())
	}
}

parameter_types! {
	pub static TrappedAssets: Vec<(MultiLocation, MultiAssets)> = vec![];
}

pub struct TestAssetTrap;

impl DropAssets for TestAssetTrap {
	fn drop_assets(origin: &MultiLocation, assets: Assets, _context: &XcmContext) -> Weight {
		let mut t: Vec<(MultiLocation, MultiAssets)> = TrappedAssets::get();
		t.push((*origin, assets.into()));
		TrappedAssets::set(t);
		Weight::from_parts(5, 5)
	}
}

impl ClaimAssets for TestAssetTrap {
	fn claim_assets(
		origin: &MultiLocation,
		ticket: &MultiLocation,
		what: &MultiAssets,
		_context: &XcmContext,
	) -> bool {
		let mut t: Vec<(MultiLocation, MultiAssets)> = TrappedAssets::get();
		if let (0, X1(GeneralIndex(i))) = (ticket.parents, &ticket.interior) {
			if let Some((l, a)) = t.get(*i as usize) {
				if l == origin && a == what {
					t.swap_remove(*i as usize);
					TrappedAssets::set(t);
					return true
				}
			}
		}
		false
	}
}

pub struct TestAssetExchanger;

impl AssetExchange for TestAssetExchanger {
	fn exchange_asset(
		_origin: Option<&MultiLocation>,
		_give: Assets,
		want: &MultiAssets,
		_maximal: bool,
	) -> Result<Assets, Assets> {
		Ok(want.clone().into())
	}
}

pub struct TestPalletsInfo;
impl PalletsInfoAccess for TestPalletsInfo {
	fn count() -> usize {
		2
	}
	fn infos() -> Vec<PalletInfoData> {
		vec![
			PalletInfoData {
				index: 0,
				name: "System",
				module_name: "pallet_system",
				crate_version: CrateVersion { major: 1, minor: 10, patch: 1 },
			},
			PalletInfoData {
				index: 1,
				name: "Balances",
				module_name: "pallet_balances",
				crate_version: CrateVersion { major: 1, minor: 42, patch: 69 },
			},
		]
	}
}

pub struct TestUniversalAliases;
impl Contains<(MultiLocation, Junction)> for TestUniversalAliases {
	fn contains(aliases: &(MultiLocation, Junction)) -> bool {
		&aliases.0 == &Here.into_location() && &aliases.1 == &GlobalConsensus(ByGenesis([0; 32]))
	}
}

parameter_types! {
	pub static LockedAssets: Vec<(MultiLocation, MultiAsset)> = vec![];
}

pub struct TestLockTicket(MultiLocation, MultiAsset);
impl Enact for TestLockTicket {
	fn enact(self) -> Result<(), LockError> {
		let mut locked_assets = LockedAssets::get();
		locked_assets.push((self.0, self.1));
		LockedAssets::set(locked_assets);
		Ok(())
	}
}
pub struct TestUnlockTicket(MultiLocation, MultiAsset);
impl Enact for TestUnlockTicket {
	fn enact(self) -> Result<(), LockError> {
		let mut locked_assets = LockedAssets::get();
		if let Some((idx, _)) = locked_assets
			.iter()
			.enumerate()
			.find(|(_, (origin, asset))| origin == &self.0 && asset == &self.1)
		{
			locked_assets.remove(idx);
		}
		LockedAssets::set(locked_assets);
		Ok(())
	}
}
pub struct TestReduceTicket;
impl Enact for TestReduceTicket {
	fn enact(self) -> Result<(), LockError> {
		Ok(())
	}
}

pub struct TestAssetLocker;
impl AssetLock for TestAssetLocker {
	type LockTicket = TestLockTicket;
	type UnlockTicket = TestUnlockTicket;
	type ReduceTicket = TestReduceTicket;

	fn prepare_lock(
		unlocker: MultiLocation,
		asset: MultiAsset,
		_owner: MultiLocation,
	) -> Result<TestLockTicket, LockError> {
		Ok(TestLockTicket(unlocker, asset))
	}

	fn prepare_unlock(
		unlocker: MultiLocation,
		asset: MultiAsset,
		_owner: MultiLocation,
	) -> Result<TestUnlockTicket, LockError> {
		Ok(TestUnlockTicket(unlocker, asset))
	}

	fn note_unlockable(
		_locker: MultiLocation,
		_asset: MultiAsset,
		_owner: MultiLocation,
	) -> Result<(), LockError> {
		Ok(())
	}

	fn prepare_reduce_unlockable(
		_locker: MultiLocation,
		_asset: MultiAsset,
		_owner: MultiLocation,
	) -> Result<TestReduceTicket, LockError> {
		Ok(TestReduceTicket)
	}
}
