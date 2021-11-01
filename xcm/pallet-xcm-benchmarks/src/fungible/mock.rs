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

//! A mock runtime for XCM benchmarking.

use crate::{fungible as xcm_balances_benchmark, mock::*};
use frame_support::{parameter_types, traits::Everything};
use sp_core::H256;
use sp_runtime::{
	testing::Header,
	traits::{BlakeTwo256, IdentityLookup},
	BuildStorage,
};
use xcm::latest::prelude::*;
use xcm_builder::AllowUnpaidExecutionFrom;

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
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(1024);
}
impl frame_system::Config for Test {
	type BaseCallFilter = Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Call = Call;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
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

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = u64;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

parameter_types! {
	pub const AssetDeposit: u64 = 100 * ExistentialDeposit::get();
	pub const ApprovalDeposit: u64 = 1 * ExistentialDeposit::get();
	pub const StringLimit: u32 = 50;
	pub const MetadataDepositBase: u64 = 10 * ExistentialDeposit::get();
	pub const MetadataDepositPerByte: u64 = 1 * ExistentialDeposit::get();
}

pub struct MatchAnyFungible;
impl xcm_executor::traits::MatchesFungible<u64> for MatchAnyFungible {
	fn matches_fungible(m: &MultiAsset) -> Option<u64> {
		use sp_runtime::traits::SaturatedConversion;
		match m {
			MultiAsset { fun: Fungible(amount), .. } => Some((*amount).saturated_into::<u64>()),
			_ => None,
		}
	}
}

// Use balances as the asset transactor.
pub type AssetTransactor = xcm_builder::CurrencyAdapter<
	Balances,
	MatchAnyFungible,
	AccountIdConverter,
	u64,
	CheckedAccount,
>;

parameter_types! {
	/// Maximum number of instructions in a single XCM fragment. A sanity check against weight
	/// calculations getting too crazy.
	pub const MaxInstructions: u32 = 100;
}

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = DevNull;
	type AssetTransactor = AssetTransactor;
	type OriginConverter = ();
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type LocationInverter = xcm_builder::LocationInverter<Ancestry>;
	type Barrier = AllowUnpaidExecutionFrom<Everything>;
	type Weigher = xcm_builder::FixedWeightBounds<UnitWeightCost, Call, MaxInstructions>;
	type Trader = xcm_builder::FixedRateOfFungible<WeightPrice, ()>;
	type ResponseHandler = DevNull;
	type AssetTrap = ();
	type AssetClaims = ();
	type SubscriptionService = ();
}

impl crate::Config for Test {
	type XcmConfig = XcmConfig;
	type AccountIdConverter = AccountIdConverter;
	fn valid_destination() -> Result<MultiLocation, sp_runtime::DispatchError> {
		let valid_destination: MultiLocation =
			X1(AccountId32 { network: NetworkId::Any, id: [0u8; 32] }).into();

		Ok(valid_destination)
	}
}

pub type TrustedTeleporters = (xcm_builder::Case<TeleConcreteFung>,);

parameter_types! {
	pub const CheckedAccount: Option<u64> = Some(100);
	pub const ChildTeleporter: MultiLocation = Parachain(1000).into();
	pub const TrustedTeleporter: Option<(MultiLocation, MultiAsset)> = Some((
		ChildTeleporter::get(),
		MultiAsset { id: Concrete(Here.into()), fun: Fungible(100) },
	));
	pub const TeleConcreteFung: (MultiAssetFilter, MultiLocation) =
		(Wild(AllOf { fun: WildFungible, id: Concrete(Here.into()) }), ChildTeleporter::get());
}

impl xcm_balances_benchmark::Config for Test {
	type TransactAsset = Balances;
	type CheckedAccount = CheckedAccount;
	type TrustedTeleporter = TrustedTeleporter;

	fn get_multi_asset() -> MultiAsset {
		let amount =
			<Balances as frame_support::traits::fungible::Inspect<u64>>::minimum_balance() as u128;
		MultiAsset { id: Concrete(Here.into()), fun: Fungible(amount) }
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = GenesisConfig { ..Default::default() }.build_storage().unwrap();
	sp_tracing::try_init_simple();
	t.into()
}
