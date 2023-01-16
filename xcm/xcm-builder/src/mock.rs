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

use crate::{barriers::AllowSubscriptionsFrom, test_utils::*};
pub use crate::{
	AllowKnownQueryResponses, AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom,
	FixedRateOfFungible, FixedWeightBounds, LocationInverter, TakeWeightCredit,
};
pub use frame_support::{
	dispatch::{
		DispatchError, DispatchInfo, DispatchResultWithPostInfo, Dispatchable, GetDispatchInfo,
		Parameter, PostDispatchInfo,
	},
	ensure, parameter_types,
	sp_runtime::DispatchErrorWithPostInfo,
	traits::{Contains, Get, IsInVec},
};
pub use parity_scale_codec::{Decode, Encode};
pub use sp_std::{
	cell::RefCell,
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	fmt::Debug,
	marker::PhantomData,
};
pub use xcm::latest::{prelude::*, Weight};
pub use xcm_executor::{
	traits::{ConvertOrigin, FilterAssetLocation, InvertLocation, OnResponse, TransactAsset},
	Assets, Config,
};

pub enum TestOrigin {
	Root,
	Relay,
	Signed(u64),
	Parachain(u32),
}

/// A dummy call.
///
/// Each item contains the amount of weight that it *wants* to consume as the first item, and the actual amount (if
/// different from the former) in the second option.
#[derive(Debug, Encode, Decode, Eq, PartialEq, Clone, Copy, scale_info::TypeInfo)]
pub enum TestCall {
	OnlyRoot(Weight, Option<Weight>),
	OnlyParachain(Weight, Option<Weight>, Option<u32>),
	OnlySigned(Weight, Option<Weight>, Option<u64>),
	Any(Weight, Option<Weight>),
}
impl Dispatchable for TestCall {
	type RuntimeOrigin = TestOrigin;
	type Config = ();
	type Info = ();
	type PostInfo = PostDispatchInfo;
	fn dispatch(self, origin: Self::RuntimeOrigin) -> DispatchResultWithPostInfo {
		let mut post_info = PostDispatchInfo::default();
		let maybe_actual = match self {
			TestCall::OnlyRoot(_, maybe_actual) |
			TestCall::OnlySigned(_, maybe_actual, _) |
			TestCall::OnlyParachain(_, maybe_actual, _) |
			TestCall::Any(_, maybe_actual) => maybe_actual,
		};
		post_info.actual_weight =
			maybe_actual.map(|x| frame_support::weights::Weight::from_ref_time(x));
		if match (&origin, &self) {
			(TestOrigin::Parachain(i), TestCall::OnlyParachain(_, _, Some(j))) => i == j,
			(TestOrigin::Signed(i), TestCall::OnlySigned(_, _, Some(j))) => i == j,
			(TestOrigin::Root, TestCall::OnlyRoot(..)) |
			(TestOrigin::Parachain(_), TestCall::OnlyParachain(_, _, None)) |
			(TestOrigin::Signed(_), TestCall::OnlySigned(_, _, None)) |
			(_, TestCall::Any(..)) => true,
			_ => false,
		} {
			Ok(post_info)
		} else {
			Err(DispatchErrorWithPostInfo { error: DispatchError::BadOrigin, post_info })
		}
	}
}

impl GetDispatchInfo for TestCall {
	fn get_dispatch_info(&self) -> DispatchInfo {
		let weight = *match self {
			TestCall::OnlyRoot(estimate, ..) |
			TestCall::OnlyParachain(estimate, ..) |
			TestCall::OnlySigned(estimate, ..) |
			TestCall::Any(estimate, ..) => estimate,
		};
		DispatchInfo {
			weight: frame_support::weights::Weight::from_ref_time(weight),
			..Default::default()
		}
	}
}

thread_local! {
	pub static SENT_XCM: RefCell<Vec<(MultiLocation, opaque::Xcm)>> = RefCell::new(Vec::new());
}
pub fn sent_xcm() -> Vec<(MultiLocation, opaque::Xcm)> {
	SENT_XCM.with(|q| (*q.borrow()).clone())
}
pub struct TestSendXcm;
impl SendXcm for TestSendXcm {
	fn send_xcm(dest: impl Into<MultiLocation>, msg: opaque::Xcm) -> SendResult {
		SENT_XCM.with(|q| q.borrow_mut().push((dest.into(), msg)));
		Ok(())
	}
}

thread_local! {
	pub static ASSETS: RefCell<BTreeMap<u64, Assets>> = RefCell::new(BTreeMap::new());
}
pub fn assets(who: u64) -> Vec<MultiAsset> {
	ASSETS.with(|a| a.borrow().get(&who).map_or(vec![], |a| a.clone().into()))
}
pub fn add_asset<AssetArg: Into<MultiAsset>>(who: u64, what: AssetArg) {
	ASSETS.with(|a| a.borrow_mut().entry(who).or_insert(Assets::new()).subsume(what.into()));
}

pub struct TestAssetTransactor;
impl TransactAsset for TestAssetTransactor {
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> Result<(), XcmError> {
		let who = to_account(who.clone()).map_err(|_| XcmError::LocationCannotHold)?;
		add_asset(who, what.clone());
		Ok(())
	}

	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<Assets, XcmError> {
		let who = to_account(who.clone()).map_err(|_| XcmError::LocationCannotHold)?;
		ASSETS.with(|a| {
			a.borrow_mut()
				.get_mut(&who)
				.ok_or(XcmError::NotWithdrawable)?
				.try_take(what.clone().into())
				.map_err(|_| XcmError::NotWithdrawable)
		})
	}
}

pub fn to_account(l: MultiLocation) -> Result<u64, MultiLocation> {
	Ok(match l {
		// Siblings at 2000+id
		MultiLocation { parents: 1, interior: X1(Parachain(id)) } => 2000 + id as u64,
		// Accounts are their number
		MultiLocation { parents: 0, interior: X1(AccountIndex64 { index, .. }) } => index,
		// Children at 1000+id
		MultiLocation { parents: 0, interior: X1(Parachain(id)) } => 1000 + id as u64,
		// Self at 3000
		MultiLocation { parents: 0, interior: Here } => 3000,
		// Parent at 3001
		MultiLocation { parents: 1, interior: Here } => 3001,
		_ => return Err(l),
	})
}

pub struct TestOriginConverter;
impl ConvertOrigin<TestOrigin> for TestOriginConverter {
	fn convert_origin(
		origin: impl Into<MultiLocation>,
		kind: OriginKind,
	) -> Result<TestOrigin, MultiLocation> {
		use OriginKind::*;
		match (kind, origin.into()) {
			(Superuser, _) => Ok(TestOrigin::Root),
			(SovereignAccount, l) => Ok(TestOrigin::Signed(to_account(l)?)),
			(Native, MultiLocation { parents: 0, interior: X1(Parachain(id)) }) =>
				Ok(TestOrigin::Parachain(id)),
			(Native, MultiLocation { parents: 1, interior: Here }) => Ok(TestOrigin::Relay),
			(Native, MultiLocation { parents: 0, interior: X1(AccountIndex64 { index, .. }) }) =>
				Ok(TestOrigin::Signed(index)),
			(_, origin) => Err(origin),
		}
	}
}

thread_local! {
	pub static IS_RESERVE: RefCell<BTreeMap<MultiLocation, Vec<MultiAssetFilter>>> = RefCell::new(BTreeMap::new());
	pub static IS_TELEPORTER: RefCell<BTreeMap<MultiLocation, Vec<MultiAssetFilter>>> = RefCell::new(BTreeMap::new());
}
pub fn add_reserve(from: MultiLocation, asset: MultiAssetFilter) {
	IS_RESERVE.with(|r| r.borrow_mut().entry(from).or_default().push(asset));
}
#[allow(dead_code)]
pub fn add_teleporter(from: MultiLocation, asset: MultiAssetFilter) {
	IS_TELEPORTER.with(|r| r.borrow_mut().entry(from).or_default().push(asset));
}
pub struct TestIsReserve;
impl FilterAssetLocation for TestIsReserve {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		IS_RESERVE
			.with(|r| r.borrow().get(origin).map_or(false, |v| v.iter().any(|a| a.contains(asset))))
	}
}
pub struct TestIsTeleporter;
impl FilterAssetLocation for TestIsTeleporter {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		IS_TELEPORTER
			.with(|r| r.borrow().get(origin).map_or(false, |v| v.iter().any(|a| a.contains(asset))))
	}
}

use xcm::latest::Response;
pub enum ResponseSlot {
	Expecting(MultiLocation),
	Received(Response),
}
thread_local! {
	pub static QUERIES: RefCell<BTreeMap<u64, ResponseSlot>> = RefCell::new(BTreeMap::new());
}
pub struct TestResponseHandler;
impl OnResponse for TestResponseHandler {
	fn expecting_response(origin: &MultiLocation, query_id: u64) -> bool {
		QUERIES.with(|q| match q.borrow().get(&query_id) {
			Some(ResponseSlot::Expecting(ref l)) => l == origin,
			_ => false,
		})
	}
	fn on_response(
		_origin: &MultiLocation,
		query_id: u64,
		response: xcm::latest::Response,
		_max_weight: Weight,
	) -> Weight {
		QUERIES.with(|q| {
			q.borrow_mut().entry(query_id).and_modify(|v| {
				if matches!(*v, ResponseSlot::Expecting(..)) {
					*v = ResponseSlot::Received(response);
				}
			});
		});
		10
	}
}
pub fn expect_response(query_id: u64, from: MultiLocation) {
	QUERIES.with(|q| q.borrow_mut().insert(query_id, ResponseSlot::Expecting(from)));
}
pub fn response(query_id: u64) -> Option<Response> {
	QUERIES.with(|q| {
		q.borrow().get(&query_id).and_then(|v| match v {
			ResponseSlot::Received(r) => Some(r.clone()),
			_ => None,
		})
	})
}

parameter_types! {
	pub TestAncestry: MultiLocation = X1(Parachain(42)).into();
	pub UnitWeightCost: u64 = 10;
}
parameter_types! {
	// Nothing is allowed to be paid/unpaid by default.
	pub static AllowUnpaidFrom: Vec<MultiLocation> = vec![];
	pub static AllowPaidFrom: Vec<MultiLocation> = vec![];
	pub static AllowSubsFrom: Vec<MultiLocation> = vec![];
	// 1_000_000_000_000 => 1 unit of asset for 1 unit of Weight.
	pub static WeightPrice: (AssetId, u128) = (From::from(Here), 1_000_000_000_000);
	pub static MaxInstructions: u32 = 100;
}

pub type TestBarrier = (
	TakeWeightCredit,
	AllowKnownQueryResponses<TestResponseHandler>,
	AllowTopLevelPaidExecutionFrom<IsInVec<AllowPaidFrom>>,
	AllowUnpaidExecutionFrom<IsInVec<AllowUnpaidFrom>>,
	AllowSubscriptionsFrom<IsInVec<AllowSubsFrom>>,
);

pub struct TestConfig;
impl Config for TestConfig {
	type RuntimeCall = TestCall;
	type XcmSender = TestSendXcm;
	type AssetTransactor = TestAssetTransactor;
	type OriginConverter = TestOriginConverter;
	type IsReserve = TestIsReserve;
	type IsTeleporter = TestIsTeleporter;
	type LocationInverter = LocationInverter<TestAncestry>;
	type Barrier = TestBarrier;
	type Weigher = FixedWeightBounds<UnitWeightCost, TestCall, MaxInstructions>;
	type Trader = FixedRateOfFungible<WeightPrice, ()>;
	type ResponseHandler = TestResponseHandler;
	type AssetTrap = TestAssetTrap;
	type AssetClaims = TestAssetTrap;
	type SubscriptionService = TestSubscriptionService;
}
