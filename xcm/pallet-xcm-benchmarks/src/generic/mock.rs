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

//! A mock runtime for XCM benchmarking.

use crate::{generic, mock::*, *};
use codec::Decode;
use frame_support::{
	match_types, parameter_types,
	traits::{Everything, OriginTrait},
	weights::Weight,
};
use sp_core::H256;
use sp_runtime::traits::{BlakeTwo256, IdentityLookup, TrailingZeroInput};
use xcm_builder::{
	test_utils::{
		Assets, TestAssetExchanger, TestAssetLocker, TestAssetTrap, TestSubscriptionService,
		TestUniversalAliases,
	},
	AliasForeignAccountId32, AllowUnpaidExecutionFrom,
};
use xcm_executor::traits::ConvertOrigin;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system::{Pallet, Call, Config<T>, Storage, Event<T>},
		XcmGenericBenchmarks: generic::{Pallet},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(Weight::from_parts(1024, u64::MAX));
}

impl frame_system::Config for Test {
	type BaseCallFilter = Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type RuntimeOrigin = RuntimeOrigin;
	type Nonce = u64;
	type Hash = H256;
	type RuntimeCall = RuntimeCall;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

/// The benchmarks in this pallet should never need an asset transactor to begin with.
pub struct NoAssetTransactor;
impl xcm_executor::traits::TransactAsset for NoAssetTransactor {
	fn deposit_asset(_: &MultiAsset, _: &MultiLocation, _: &XcmContext) -> Result<(), XcmError> {
		unreachable!();
	}

	fn withdraw_asset(
		_: &MultiAsset,
		_: &MultiLocation,
		_: Option<&XcmContext>,
	) -> Result<Assets, XcmError> {
		unreachable!();
	}
}

parameter_types! {
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
}

match_types! {
	pub type OnlyParachains: impl Contains<MultiLocation> = {
		MultiLocation { parents: 0, interior: X1(Parachain(_)) }
	};
}

type Aliasers = AliasForeignAccountId32<OnlyParachains>;
pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = DevNull;
	type AssetTransactor = NoAssetTransactor;
	type OriginConverter = AlwaysSignedByDefault<RuntimeOrigin>;
	type IsReserve = AllAssetLocationsPass;
	type IsTeleporter = ();
	type UniversalLocation = UniversalLocation;
	type Barrier = AllowUnpaidExecutionFrom<Everything>;
	type Weigher = xcm_builder::FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
	type Trader = xcm_builder::FixedRateOfFungible<WeightPrice, ()>;
	type ResponseHandler = DevNull;
	type AssetTrap = TestAssetTrap;
	type AssetLocker = TestAssetLocker;
	type AssetExchanger = TestAssetExchanger;
	type AssetClaims = TestAssetTrap;
	type SubscriptionService = TestSubscriptionService;
	type PalletInstancesInfo = AllPalletsWithSystem;
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type FeeManager = ();
	// No bridges yet...
	type MessageExporter = ();
	type UniversalAliases = TestUniversalAliases;
	type CallDispatcher = RuntimeCall;
	type SafeCallFilter = Everything;
	type Aliasers = Aliasers;
}

impl crate::Config for Test {
	type XcmConfig = XcmConfig;
	type AccountIdConverter = AccountIdConverter;
	fn valid_destination() -> Result<MultiLocation, BenchmarkError> {
		let valid_destination: MultiLocation =
			Junction::AccountId32 { network: None, id: [0u8; 32] }.into();

		Ok(valid_destination)
	}
	fn worst_case_holding(depositable_count: u32) -> MultiAssets {
		crate::mock_worst_case_holding(
			depositable_count,
			<XcmConfig as xcm_executor::Config>::MaxAssetsIntoHolding::get(),
		)
	}
}

impl generic::Config for Test {
	type RuntimeCall = RuntimeCall;

	fn worst_case_response() -> (u64, Response) {
		let assets: MultiAssets = (Concrete(Here.into()), 100).into();
		(0, Response::Assets(assets))
	}

	fn worst_case_asset_exchange() -> Result<(MultiAssets, MultiAssets), BenchmarkError> {
		Ok(Default::default())
	}

	fn universal_alias() -> Result<(MultiLocation, Junction), BenchmarkError> {
		Ok((Here.into(), GlobalConsensus(ByGenesis([0; 32]))))
	}

	fn transact_origin_and_runtime_call(
	) -> Result<(MultiLocation, <Self as generic::Config>::RuntimeCall), BenchmarkError> {
		Ok((Default::default(), frame_system::Call::remark_with_event { remark: vec![] }.into()))
	}

	fn subscribe_origin() -> Result<MultiLocation, BenchmarkError> {
		Ok(Default::default())
	}

	fn claimable_asset() -> Result<(MultiLocation, MultiLocation, MultiAssets), BenchmarkError> {
		let assets: MultiAssets = (Concrete(Here.into()), 100).into();
		let ticket = MultiLocation { parents: 0, interior: X1(GeneralIndex(0)) };
		Ok((Default::default(), ticket, assets))
	}

	fn unlockable_asset() -> Result<(MultiLocation, MultiLocation, MultiAsset), BenchmarkError> {
		let assets: MultiAsset = (Concrete(Here.into()), 100).into();
		Ok((Default::default(), Default::default(), assets))
	}

	fn export_message_origin_and_destination(
	) -> Result<(MultiLocation, NetworkId, InteriorMultiLocation), BenchmarkError> {
		// No MessageExporter in tests
		Err(BenchmarkError::Skip)
	}

	fn alias_origin() -> Result<(MultiLocation, MultiLocation), BenchmarkError> {
		let origin: MultiLocation =
			(Parachain(1), AccountId32 { network: None, id: [0; 32] }).into();
		let target: MultiLocation = AccountId32 { network: None, id: [0; 32] }.into();
		Ok((origin, target))
	}
}

#[cfg(feature = "runtime-benchmarks")]
pub fn new_test_ext() -> sp_io::TestExternalities {
	use sp_runtime::BuildStorage;
	let t = RuntimeGenesisConfig { ..Default::default() }.build_storage().unwrap();
	sp_tracing::try_init_simple();
	t.into()
}

pub struct AlwaysSignedByDefault<RuntimeOrigin>(core::marker::PhantomData<RuntimeOrigin>);
impl<RuntimeOrigin> ConvertOrigin<RuntimeOrigin> for AlwaysSignedByDefault<RuntimeOrigin>
where
	RuntimeOrigin: OriginTrait,
	<RuntimeOrigin as OriginTrait>::AccountId: Decode,
{
	fn convert_origin(
		_origin: impl Into<MultiLocation>,
		_kind: OriginKind,
	) -> Result<RuntimeOrigin, MultiLocation> {
		Ok(RuntimeOrigin::signed(
			<RuntimeOrigin as OriginTrait>::AccountId::decode(&mut TrailingZeroInput::zeroes())
				.expect("infinite length input; no invalid inputs for type; qed"),
		))
	}
}
