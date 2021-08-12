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
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use frame_support::{
	construct_runtime, parameter_types,
	traits::Everything,
	weights::Weight,
};
use sp_core::H256;
use sp_runtime::{testing::Header, traits::IdentityLookup, AccountId32};

use polkadot_parachain::primitives::Id as ParaId;
use polkadot_runtime_parachains::{configuration, origin, shared, ump};
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowTopLevelPaidExecutionFrom, ChildParachainAsNative,
	ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfFungible, FixedWeightBounds,
	IsConcrete, LocationInverter, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation, TakeWeightCredit, IsChildSystemParachain, AllowUnpaidExecutionFrom
};
use xcm_executor::XcmExecutor;

pub type AccountId = AccountId32;
pub type Balance = u128;

// copied from kusama constants
pub const UNITS: Balance = 1_000_000_000_000;
pub const CENTS: Balance = UNITS / 30_000;

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Runtime {
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = ::sp_runtime::traits::BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type BlockWeights = ();
	type BlockLength = ();
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type DbWeight = ();
	type BaseCallFilter = Everything;
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

parameter_types! {
	pub ExistentialDeposit: Balance = 1 * CENTS;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = MaxLocks;
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
}

impl shared::Config for Runtime {}

impl configuration::Config for Runtime {}

// aims to closely emulate the Kusama XcmConfig
parameter_types! {
	pub const KsmLocation: MultiLocation = Here.into();
	pub const KusamaNetwork: NetworkId = NetworkId::Kusama;
	pub Ancestry: MultiLocation = Here.into();
	pub CheckAccount: AccountId = XcmPallet::check_account();
}

pub type SovereignAccountOf =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<KusamaNetwork, AccountId>);

pub type LocalAssetTransactor =
	XcmCurrencyAdapter<Balances, IsConcrete<KsmLocation>, SovereignAccountOf, AccountId, CheckAccount>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<SovereignAccountOf, Origin>,
	ChildParachainAsNative<origin::Origin, Origin>,
	SignedAccountId32AsNative<KusamaNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

parameter_types! {
	pub const BaseXcmWeight: Weight = 1_000_000_000;
	pub KsmPerSecond: (AssetId, u128) = (Concrete(KsmLocation::get()), 1);
}

parameter_types! {
	pub const Kusama: MultiAssetFilter = Wild(AllOf { fun: WildFungible, id: Concrete(KsmLocation::get()) });
	pub const KusamaForStatemint: (MultiAssetFilter, MultiLocation) =
		(Kusama::get(), Parachain(1000).into());
}
pub type TrustedTeleporters = (
	xcm_builder::Case<KusamaForStatemint>,
);

pub type Barrier = (
	TakeWeightCredit,
	AllowTopLevelPaidExecutionFrom<Everything>,
	// Unused/Untested
	AllowUnpaidExecutionFrom<IsChildSystemParachain<ParaId>>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = super::RelayChainXcmRouter;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type LocationInverter = LocationInverter<Ancestry>;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
	type Trader = FixedRateOfFungible<KsmPerSecond, ()>;
	type ResponseHandler = ();
}

pub type LocalOriginToLocation = SignedToAccountId32<Origin, AccountId, KusamaNetwork>;

impl pallet_xcm::Config for Runtime {
	type Event = Event;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = super::RelayChainXcmRouter;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	// ...but they must match our filter, which requires them to be a simple withdraw + teleport.
	type XcmExecuteFilter = ();
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
	type LocationInverter = LocationInverter<Ancestry>;
}

parameter_types! {
	pub const FirstMessageFactorPercent: u64 = 100;
}

impl ump::Config for Runtime {
	type Event = Event;
	type UmpSink = ump::XcmSink<XcmExecutor<XcmConfig>, Runtime>;
	type FirstMessageFactorPercent = FirstMessageFactorPercent;
}

impl origin::Config for Runtime {}

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Runtime>;
type Block = frame_system::mocking::MockBlock<Runtime>;

construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Storage, Config, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		ParasOrigin: origin::{Pallet, Origin},
		ParasUmp: ump::{Pallet, Call, Storage, Event},
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>},
	}
);
