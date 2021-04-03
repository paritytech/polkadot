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

use super::*;
pub use sp_std::{fmt::Debug, marker::PhantomData, cell::RefCell};
pub use sp_std::collections::{btree_map::BTreeMap, btree_set::BTreeSet};
pub use parity_scale_codec::{Encode, Decode};
pub use xcm::v0::{
	SendXcm, MultiLocation::*, Junction::*, MultiAsset, XcmGeneric, OrderGeneric, Result as XcmResult, Error,
	OriginKind, MultiLocation, Junction,
};
pub use frame_support::{
	ensure, parameter_types,
	dispatch::{Dispatchable, Parameter, Weight, DispatchError, DispatchResultWithPostInfo, DispatchInfo},
	weights::{PostDispatchInfo, GetDispatchInfo},
	sp_runtime::DispatchErrorWithPostInfo,
	traits::{Get, Contains},
};
pub use crate::traits::{TransactAsset, ConvertOrigin, FilterAssetLocation, InvertLocation, LocationInverter};
pub use crate::config::{
	TakeWeightCredit, AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom, FixedWeightBounds,
	FixedRateOfConcreteFungible,
};

pub enum TestOrigin { Root, Signed(u64), Parachain(u32) }

#[derive(Debug, Encode, Decode, Eq, PartialEq, Clone, Copy)]
pub enum TestCall {
	OnlyRoot(Weight, Option<Weight>),
	OnlyParachain(Weight, Option<Weight>, Option<u32>),
	OnlySigned(Weight, Option<Weight>, Option<u64>),
	Any(Weight, Option<Weight>),
}
impl Dispatchable for TestCall {
	type Origin = TestOrigin;
	type Config = ();
	type Info = ();
	type PostInfo = PostDispatchInfo;
	fn dispatch(self, origin: Self::Origin) -> DispatchResultWithPostInfo {
		let mut post_info = PostDispatchInfo::default();
		post_info.actual_weight = match self {
			TestCall::OnlyRoot(_, maybe_actual)
			| TestCall::OnlySigned(_, maybe_actual, _)
			| TestCall::OnlyParachain(_, maybe_actual, _)
			| TestCall::Any(_, maybe_actual)
			=> maybe_actual,
		};
		if match (&origin, &self) {
			(TestOrigin::Parachain(i), TestCall::OnlyParachain(_, _, Some(j)))
			=> i == j,
			(TestOrigin::Signed(i), TestCall::OnlySigned(_, _, Some(j)))
			=> i == j,

			(TestOrigin::Root, TestCall::OnlyRoot(..))
			| (TestOrigin::Parachain(_), TestCall::OnlyParachain(_, _, None))
			| (TestOrigin::Signed(_), TestCall::OnlySigned(_, _, None))
			| (_, TestCall::Any(..))
			=> true,

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
			TestCall::OnlyRoot(estimate, ..)
			| TestCall::OnlyParachain(estimate, ..)
			| TestCall::OnlySigned(estimate, ..)
			| TestCall::Any(estimate, ..)
			=> estimate,
		};
		DispatchInfo { weight, .. Default::default() }
	}
}

thread_local! {
	pub static SENT_XCM: RefCell<Vec<(MultiLocation, Xcm)>> = RefCell::new(Vec::new());
}
pub struct TestSendXcm;
impl SendXcm for TestSendXcm {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> XcmResult {
		SENT_XCM.with(|q| q.borrow_mut().push((dest, msg)));
		Ok(())
	}
}

thread_local! {
	pub static ASSETS: RefCell<BTreeMap<MultiLocation, Assets>> = RefCell::new(BTreeMap::new());
}
pub fn assets(who: MultiLocation) -> Vec<MultiAsset> {
	ASSETS.with(|a| a.borrow().get(&who).map_or(vec![], |a| a.clone().into()))
}
pub struct TestAssetTransactor;
impl TransactAsset for TestAssetTransactor {
	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> Result<(), XcmError> {
		ASSETS.with(|a| a.borrow_mut()
			.entry(who.clone())
			.or_insert(Assets::new())
			.saturating_subsume(what.clone())
		);
		Ok(())
	}

	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<Assets, XcmError> {
		ASSETS.with(|a| a.borrow_mut()
			.get_mut(who)
			.ok_or(XcmError::Undefined)?
			.try_take(what.clone())
			.map_err(|()| XcmError::Undefined)
		)
	}
}

pub struct TestOriginConverter;
impl ConvertOrigin<TestOrigin> for TestOriginConverter {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<TestOrigin, MultiLocation> {
		use {OriginKind::*};
		match (kind, origin) {
			(Superuser, _) => Ok(TestOrigin::Root),
			(SovereignAccount, X2(Parent, Parachain { id })) => Ok(TestOrigin::Signed(2000 + id as u64)),
			(SovereignAccount, X1(AccountIndex64 { index, .. })) => Ok(TestOrigin::Signed(index)),
			(SovereignAccount, X1(Parachain { id })) => Ok(TestOrigin::Signed(1000 + id as u64)),
			(SovereignAccount, Null) => Ok(TestOrigin::Signed(3000)),
			(Native, X1(Parachain { id })) => Ok(TestOrigin::Parachain(id)),
			(_, origin) => Err(origin),
		}
	}
}

thread_local! {
	pub static IS_RESERVE: RefCell<BTreeMap<MultiLocation, Vec<MultiAsset>>> = RefCell::new(BTreeMap::new());
	pub static IS_TELEPORTER: RefCell<BTreeMap<MultiLocation, Vec<MultiAsset>>> = RefCell::new(BTreeMap::new());
}

pub fn add_reserve(from: MultiLocation, asset: MultiAsset) {
	IS_RESERVE.with(|r| r.borrow_mut().entry(from).or_default().push(asset));
}
pub fn add_teleporter(from: MultiLocation, asset: MultiAsset) {
	IS_TELEPORTER.with(|r| r.borrow_mut().entry(from).or_default().push(asset));
}

pub struct TestIsReserve;
impl FilterAssetLocation for TestIsReserve {
	/// A filter to distinguish between asset/location pairs.
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		let r = IS_RESERVE.with(|r| r.borrow().get(origin)
			.map_or(false, |v| v.iter().any(|a| a.contains(asset)))
		);
		println!("FAL: {:?}, {:?} => {}", asset, origin, r);
		r
	}
}
pub struct TestIsTeleporter;
impl FilterAssetLocation for TestIsTeleporter {
	/// A filter to distinguish between asset/location pairs.
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		IS_TELEPORTER.with(|r| r.borrow().get(origin)
			.map_or(false, |v| v.iter().any(|a| a.contains(asset)))
		)
	}
}

parameter_types! {
	pub TestAncestry: MultiLocation = X1(Parachain{id: 42});
	pub AllowPaidFrom: Vec<MultiLocation> = vec![
		Null,							// this chain
		X1(Parent),						// the relay chain
		X2(Parent, Parachain{id: 69}),	// our sibling chain 69
	];
	pub AllowUnpaidFrom: Vec<MultiLocation> = vec![X1(Parent)];
	pub UnitWeightCost: Weight = 10;
	pub WeightPrice: (MultiLocation, u128) = (X1(Parent), 1_000_000_000_000);
}

pub struct IsInVec<T>(PhantomData<T>);
impl<X: Ord + PartialOrd, T: Get<Vec<X>>> Contains<X> for IsInVec<T> {
	fn sorted_members() -> Vec<X> { let mut r = T::get(); r.sort(); r }
}

pub type TestBarrier = (
	TakeWeightCredit,
	AllowTopLevelPaidExecutionFrom<IsInVec<AllowPaidFrom>>,
	AllowUnpaidExecutionFrom<IsInVec<AllowUnpaidFrom>>,
);

pub struct TestConfig;
impl Config for TestConfig {
	type Call = TestCall;
	type XcmSender = TestSendXcm;
	type AssetTransactor = TestAssetTransactor;
	type OriginConverter = TestOriginConverter;
	type IsReserve = TestIsReserve;
	type IsTeleporter = TestIsTeleporter;
	type LocationInverter = LocationInverter<TestAncestry>;
	type Barrier = TestBarrier;
	type Weigher = FixedWeightBounds<UnitWeightCost, TestCall>;
	type Trader = FixedRateOfConcreteFungible<WeightPrice>;
}
