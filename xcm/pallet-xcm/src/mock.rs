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

use frame_support::{construct_runtime, parameter_types, traits::Everything};
use polkadot_parachain::primitives::Id as ParaId;
use polkadot_runtime_parachains::origin;
use sp_core::H256;
use sp_runtime::{testing::Header, traits::IdentityLookup, AccountId32};
pub use sp_std::{cell::RefCell, fmt::Debug, marker::PhantomData};
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowKnownQueryResponses, AllowSubscriptionsFrom,
	AllowTopLevelPaidExecutionFrom, Case, ChildParachainAsNative, ChildParachainConvertsVia,
	ChildSystemParachainAsSuperuser, CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfFungible,
	FixedWeightBounds, IsConcrete, LocationInverter, SignedAccountId32AsNative,
	SignedToAccountId32, SovereignSignedViaLocation, TakeWeightCredit,
};
use xcm_executor::XcmExecutor;

use crate as pallet_xcm;

pub type AccountId = AccountId32;
pub type Balance = u128;
type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

#[frame_support::pallet]
pub mod pallet_test_notifier {
	use crate::{ensure_response, QueryId};
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use sp_runtime::DispatchResult;
	use xcm::latest::prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + crate::Config {
		type RuntimeEvent: IsType<<Self as frame_system::Config>::RuntimeEvent> + From<Event<Self>>;
		type RuntimeOrigin: IsType<<Self as frame_system::Config>::RuntimeOrigin>
			+ Into<Result<crate::Origin, <Self as Config>::RuntimeOrigin>>;
		type RuntimeCall: IsType<<Self as crate::Config>::RuntimeCall> + From<Call<Self>>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		QueryPrepared(QueryId),
		NotifyQueryPrepared(QueryId),
		ResponseReceived(MultiLocation, QueryId, Response),
	}

	#[pallet::error]
	pub enum Error<T> {
		UnexpectedId,
		BadAccountFormat,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(1_000_000)]
		pub fn prepare_new_query(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let id = who
				.using_encoded(|mut d| <[u8; 32]>::decode(&mut d))
				.map_err(|_| Error::<T>::BadAccountFormat)?;
			let qid = crate::Pallet::<T>::new_query(
				Junction::AccountId32 { network: Any, id }.into(),
				100u32.into(),
			);
			Self::deposit_event(Event::<T>::QueryPrepared(qid));
			Ok(())
		}

		#[pallet::weight(1_000_000)]
		pub fn prepare_new_notify_query(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let id = who
				.using_encoded(|mut d| <[u8; 32]>::decode(&mut d))
				.map_err(|_| Error::<T>::BadAccountFormat)?;
			let call =
				Call::<T>::notification_received { query_id: 0, response: Default::default() };
			let qid = crate::Pallet::<T>::new_notify_query(
				Junction::AccountId32 { network: Any, id }.into(),
				<T as Config>::RuntimeCall::from(call),
				100u32.into(),
			);
			Self::deposit_event(Event::<T>::NotifyQueryPrepared(qid));
			Ok(())
		}

		#[pallet::weight(1_000_000)]
		pub fn notification_received(
			origin: OriginFor<T>,
			query_id: QueryId,
			response: Response,
		) -> DispatchResult {
			let responder = ensure_response(<T as Config>::RuntimeOrigin::from(origin))?;
			Self::deposit_event(Event::<T>::ResponseReceived(responder, query_id, response));
			Ok(())
		}
	}
}

construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Storage, Config, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		ParasOrigin: origin::{Pallet, Origin},
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>, Origin, Config},
		TestNotifier: pallet_test_notifier::{Pallet, Call, Event<T>},
	}
);

thread_local! {
	pub static SENT_XCM: RefCell<Vec<(MultiLocation, Xcm<()>)>> = RefCell::new(Vec::new());
}
pub(crate) fn sent_xcm() -> Vec<(MultiLocation, Xcm<()>)> {
	SENT_XCM.with(|q| (*q.borrow()).clone())
}
pub(crate) fn take_sent_xcm() -> Vec<(MultiLocation, Xcm<()>)> {
	SENT_XCM.with(|q| {
		let mut r = Vec::new();
		std::mem::swap(&mut r, &mut *q.borrow_mut());
		r
	})
}
/// Sender that never returns error, always sends
pub struct TestSendXcm;
impl SendXcm for TestSendXcm {
	fn send_xcm(dest: impl Into<MultiLocation>, msg: Xcm<()>) -> SendResult {
		SENT_XCM.with(|q| q.borrow_mut().push((dest.into(), msg)));
		Ok(())
	}
}
/// Sender that returns error if `X8` junction and stops routing
pub struct TestSendXcmErrX8;
impl SendXcm for TestSendXcmErrX8 {
	fn send_xcm(dest: impl Into<MultiLocation>, msg: Xcm<()>) -> SendResult {
		let dest = dest.into();
		if dest.len() == 8 {
			Err(SendError::Transport("Destination location full"))
		} else {
			SENT_XCM.with(|q| q.borrow_mut().push((dest, msg)));
			Ok(())
		}
	}
}

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Test {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = ::sp_runtime::traits::BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type RuntimeEvent = RuntimeEvent;
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
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub ExistentialDeposit: Balance = 1;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Test {
	type MaxLocks = MaxLocks;
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
}

parameter_types! {
	pub const RelayLocation: MultiLocation = Here.into();
	pub const AnyNetwork: NetworkId = NetworkId::Any;
	pub Ancestry: MultiLocation = Here.into();
	pub UnitWeightCost: u64 = 1_000;
}

pub type SovereignAccountOf =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<AnyNetwork, AccountId>);

pub type LocalAssetTransactor =
	XcmCurrencyAdapter<Balances, IsConcrete<RelayLocation>, SovereignAccountOf, AccountId, ()>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<SovereignAccountOf, RuntimeOrigin>,
	ChildParachainAsNative<origin::Origin, RuntimeOrigin>,
	SignedAccountId32AsNative<AnyNetwork, RuntimeOrigin>,
	ChildSystemParachainAsSuperuser<ParaId, RuntimeOrigin>,
);

parameter_types! {
	pub const BaseXcmWeight: u64 = 1_000;
	pub CurrencyPerSecond: (AssetId, u128) = (Concrete(RelayLocation::get()), 1);
	pub TrustedAssets: (MultiAssetFilter, MultiLocation) = (All.into(), Here.into());
	pub const MaxInstructions: u32 = 100;
}

pub type Barrier = (
	TakeWeightCredit,
	AllowTopLevelPaidExecutionFrom<Everything>,
	AllowKnownQueryResponses<XcmPallet>,
	AllowSubscriptionsFrom<Everything>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = TestSendXcm;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = Case<TrustedAssets>;
	type LocationInverter = LocationInverter<Ancestry>;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type Trader = FixedRateOfFungible<CurrencyPerSecond, ()>;
	type ResponseHandler = XcmPallet;
	type AssetTrap = XcmPallet;
	type AssetClaims = XcmPallet;
	type SubscriptionService = XcmPallet;
}

pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, AnyNetwork>;

parameter_types! {
	pub static AdvertisedXcmVersion: pallet_xcm::XcmVersion = 2;
}

impl pallet_xcm::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = (TestSendXcmErrX8, TestSendXcm);
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type LocationInverter = LocationInverter<Ancestry>;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = AdvertisedXcmVersion;
}

impl origin::Config for Test {}

impl pallet_test_notifier::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
}

pub(crate) fn last_event() -> RuntimeEvent {
	System::events().pop().expect("RuntimeEvent expected").event
}

pub(crate) fn last_events(n: usize) -> Vec<RuntimeEvent> {
	System::events().into_iter().map(|e| e.event).rev().take(n).rev().collect()
}

pub(crate) fn buy_execution<C>(fees: impl Into<MultiAsset>) -> Instruction<C> {
	use xcm::latest::prelude::*;
	BuyExecution { fees: fees.into(), weight_limit: Unlimited }
}

pub(crate) fn buy_limited_execution<C>(fees: impl Into<MultiAsset>, weight: u64) -> Instruction<C> {
	use xcm::latest::prelude::*;
	BuyExecution { fees: fees.into(), weight_limit: Limited(weight) }
}

pub(crate) fn new_test_ext_with_balances(
	balances: Vec<(AccountId, Balance)>,
) -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();

	pallet_balances::GenesisConfig::<Test> { balances }
		.assimilate_storage(&mut t)
		.unwrap();

	<pallet_xcm::GenesisConfig as frame_support::traits::GenesisBuild<Test>>::assimilate_storage(
		&pallet_xcm::GenesisConfig { safe_xcm_version: Some(2) },
		&mut t,
	)
	.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}
