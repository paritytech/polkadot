// Copyright 2020 Parity Technologies (UK) Ltd.
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

use frame_support::dispatch::Weight;
use sp_std::cell::RefCell;
pub use xcm::latest::prelude::*;
use xcm_executor::traits::{ClaimAssets, DropAssets, VersionChangeNotifier};
pub use xcm_executor::{
	traits::{ConvertOrigin, FilterAssetLocation, InvertLocation, OnResponse, TransactAsset},
	Assets, Config,
};

thread_local! {
	pub static SUB_REQUEST: RefCell<Vec<(MultiLocation, Option<(QueryId, u64)>)>> = RefCell::new(Vec::new());
}

pub struct SubscriptionRequests;
impl SubscriptionRequests {
	pub fn get() -> Vec<(MultiLocation, Option<(QueryId, u64)>)> {
		SUB_REQUEST.with(|q| (*q.borrow()).clone())
	}
	pub fn set(r: Vec<(MultiLocation, Option<(QueryId, u64)>)>) {
		SUB_REQUEST.with(|a| a.replace(r));
	}
}

pub struct TestSubscriptionService;

impl VersionChangeNotifier for TestSubscriptionService {
	fn start(location: &MultiLocation, query_id: QueryId, max_weight: u64) -> XcmResult {
		let mut r = SubscriptionRequests::get();
		r.push((location.clone(), Some((query_id, max_weight))));
		SubscriptionRequests::set(r);
		Ok(())
	}
	fn stop(location: &MultiLocation) -> XcmResult {
		let mut r = SubscriptionRequests::get();
		r.retain(|(l, _q)| l != location);
		r.push((location.clone(), None));
		SubscriptionRequests::set(r);
		Ok(())
	}
	fn is_subscribed(location: &MultiLocation) -> bool {
		let r = SubscriptionRequests::get();
		r.iter().any(|(l, q)| l == location && q.is_some())
	}
}

thread_local! {
	pub static TRAPPED_ASSETS: RefCell<Vec<(MultiLocation, MultiAssets)>> = RefCell::new(Vec::new());
}

pub struct TrappedAssets;
impl TrappedAssets {
	pub fn get() -> Vec<(MultiLocation, MultiAssets)> {
		TRAPPED_ASSETS.with(|q| (*q.borrow()).clone())
	}
	pub fn set(r: Vec<(MultiLocation, MultiAssets)>) {
		TRAPPED_ASSETS.with(|a| a.replace(r));
	}
}

pub struct TestAssetTrap;

impl DropAssets for TestAssetTrap {
	fn drop_assets(origin: &MultiLocation, assets: Assets) -> Weight {
		let mut t: Vec<(MultiLocation, MultiAssets)> = TrappedAssets::get();
		t.push((origin.clone(), assets.into()));
		TrappedAssets::set(t);
		5
	}
}

impl ClaimAssets for TestAssetTrap {
	fn claim_assets(origin: &MultiLocation, ticket: &MultiLocation, what: &MultiAssets) -> bool {
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
