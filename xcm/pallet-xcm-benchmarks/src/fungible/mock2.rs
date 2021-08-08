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

//! A mock runtime for xcm benchmarking.

use crate::{fungible as xcm_balances_benchmark, mock::*, Weight};
use frame_support::{parameter_types, traits::All};
use polkadot_runtime_common::xcm_sender;
use sp_core::H256;
use sp_runtime::{
	testing::Header,
	traits::{BlakeTwo256, IdentityLookup},
	BuildStorage,
};

use xcm::latest::{
	Junction::Parachain,
	MultiAsset::{self, AllConcreteFungible},
	MultiLocation::{self, Null, X1},
	NetworkId, Xcm,
};
use xcm_builder::{
	AccountId32Aliases, AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom,
	ChildParachainAsNative, ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedWeightBounds, IsChildSystemParachain, IsConcrete,
	LocationInverter, SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation,
	TakeWeightCredit, UsingComponents,
};
use xcm_executor::XcmExecutor;

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

// For testing the pallet, we construct a mock runtime.
frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		XcmBalancesBenchmark: xcm_balances_benchmark::{Pallet},
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>, Origin},
	}
);

type AccountId = u64;

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(1024);
}
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::AllowAll;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Call = Call;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

parameter_types! {
	pub const ExistentialDeposit: u64 = 7;
}

type Balance = u64;

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = Balance;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<Origin, AccountId, WestendNetwork>,
);

impl pallet_xcm::Config for Test {
	type Event = Event;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	// ...but they must match our filter, which requires them to be a simple withdraw + teleport.
	type XcmExecuteFilter = OnlyWithdrawTeleportForAccounts;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type XcmReserveTransferFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
}

parameter_types! {
	pub const WndLocation: MultiLocation = MultiLocation::Null;
	pub const Ancestry: MultiLocation = MultiLocation::Null;
	pub WestendNetwork: NetworkId = NetworkId::Named(b"Westend".to_vec());
	pub CheckAccount: AccountId = XcmPallet::check_account();
}

pub struct OnlyWithdrawTeleportForAccounts;
impl frame_support::traits::Contains<(MultiLocation, Xcm<Call>)>
	for OnlyWithdrawTeleportForAccounts
{
	fn contains((ref origin, ref msg): &(MultiLocation, Xcm<Call>)) -> bool {
		use xcm::latest::{
			Junction::AccountId32,
			MultiAsset::{All, ConcreteFungible},
			Order::{BuyExecution, DepositAsset, InitiateTeleport},
			Xcm::WithdrawAsset,
		};
		match origin {
			// Root is allowed to execute anything.
			Null => true,
			X1(AccountId32 { .. }) => {
				// An account ID trying to send a message. We ensure that it's sensible.
				// This checks that it's of the form:
				// WithdrawAsset {
				//   assets: [ ConcreteFungible { id: Null } ],
				//   effects: [ BuyExecution, InitiateTeleport {
				//     assets: All,
				//     dest: Parachain,
				//     effects: [ BuyExecution, DepositAssets {
				//       assets: All,
				//       dest: AccountId32,
				//     } ]
				//   } ]
				// }
				matches!(msg, WithdrawAsset { ref assets, ref effects }
					if assets.len() == 1
					&& matches!(assets[0], ConcreteFungible { id: Null, .. })
					&& effects.len() == 2
					&& matches!(effects[0], BuyExecution { .. })
					&& matches!(effects[1], InitiateTeleport { ref assets, dest: X1(Parachain(..)), ref effects }
						if assets.len() == 1
						&& matches!(assets[0], All)
						&& effects.len() == 2
						&& matches!(effects[0], BuyExecution { .. })
						&& matches!(effects[1], DepositAsset { ref assets, dest: X1(AccountId32{..}) }
							if assets.len() == 1
							&& matches!(assets[0], All)
						)
					)
				)
			},
			// Nobody else is allowed to execute anything.
			_ => false,
		}
	}
}

pub type LocationConverter =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<WestendNetwork, AccountId>);

pub type LocalAssetTransactor = XcmCurrencyAdapter<
	// Use this currency:
	Balances,
	// Use this currency when it is a fungible asset matching the given location or name:
	IsConcrete<WndLocation>,
	// We can convert the MultiLocations with our converter above:
	LocationConverter,
	// Our chain's account ID type (we can't get away without mentioning it explicitly):
	AccountId,
	// It's a native asset so we keep track of the teleports to maintain total issuance.
	CheckAccount,
>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<LocationConverter, Origin>,
	//ChildParachainAsNative<parachains_origin::Origin, Origin>,
	SignedAccountId32AsNative<WestendNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

parameter_types! {
	pub const BaseXcmWeight: Weight = 10_000_000;
}

/// The XCM router. When we want to send an XCM message, we use this type. It amalgamates all of our
/// individual routers.
pub type XcmRouter = (
	// Only one router so far - use DMP to communicate with child parachains.
	xcm_sender::ChildParachainRouter<Runtime>,
);

parameter_types! {
	pub const WestendForWestmint: (MultiAsset, MultiLocation) =
		(AllConcreteFungible { id: Null }, X1(Parachain(1000)));
}
pub type TrustedTeleporters = (xcm_builder::Case<WestendForWestmint>,);

/// The barriers one of which must be passed for an XCM message to be executed.
pub type Barrier = (
	// Weight that is paid for may be consumed.
	TakeWeightCredit,
	// If the message is one that immediately attemps to pay for execution, then allow it.
	AllowTopLevelPaidExecutionFrom<Everything>,
	// Messages coming from system parachains need not pay for execution.
	AllowUnpaidExecutionFrom<IsChildSystemParachain<ParaId>>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = XcmRouter;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type LocationInverter = LocationInverter<Ancestry>;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
	type Trader =
		UsingComponents<IdentityFee<Balance>, WndLocation, AccountId, Balances, ToAuthor<Runtime>>;
	type ResponseHandler = ();
}

impl crate::Config for Test {
	type XcmConfig = XcmConfig;
	type AccountIdConverter = AccountIdConverter;
}

impl xcm_balances_benchmark::Config for Test {
	type TransactAsset = Balances;
	type CheckedAccount = CheckedAccount;
	fn get_multi_asset() -> MultiAsset {
		let amount =
			<Balances as frame_support::traits::fungible::Inspect<u64>>::minimum_balance() as u128;
		MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount }
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = GenesisConfig { ..Default::default() }.build_storage().unwrap();
	sp_tracing::try_init_simple();
	t.into()
}
