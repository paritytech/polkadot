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

//! Parachain runtime mock.

use codec::{Decode, Encode};
use frame_support::{
	construct_runtime, parameter_types,
	traits::{EnsureOrigin, EnsureOriginWithArg, Everything, EverythingBut, Nothing},
	weights::{constants::WEIGHT_REF_TIME_PER_SECOND, Weight},
};

use frame_system::EnsureRoot;
use sp_core::{ConstU32, H256};
use sp_runtime::{
	testing::Header,
	traits::{Hash, IdentityLookup},
	AccountId32,
};
use sp_std::prelude::*;

use pallet_xcm::XcmPassthrough;
use polkadot_core_primitives::BlockNumber as RelayBlockNumber;
use polkadot_parachain::primitives::{
	DmpMessageHandler, Id as ParaId, Sibling, XcmpMessageFormat, XcmpMessageHandler,
};
use xcm::{latest::prelude::*, VersionedXcm};
use xcm_builder::{
	Account32Hash, AccountId32Aliases, AllowUnpaidExecutionFrom, ConvertedConcreteId,
	CurrencyAdapter as XcmCurrencyAdapter, EnsureXcmOrigin, FixedRateOfFungible, FixedWeightBounds,
	IsConcrete, NativeAsset, NoChecking, NonFungiblesAdapter, ParentIsPreset,
	SiblingParachainConvertsVia, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation,
};
use xcm_executor::{
	traits::{Convert, JustTry},
	Config, XcmExecutor,
};

pub type SovereignAccountOf = (
	SiblingParachainConvertsVia<Sibling, AccountId>,
	AccountId32Aliases<RelayNetwork, AccountId>,
	ParentIsPreset<AccountId>,
);

pub type AccountId = AccountId32;
pub type Balance = u128;

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Runtime {
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
	type MaxConsumers = ConstU32<16>;
}

parameter_types! {
	pub ExistentialDeposit: Balance = 1;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = MaxLocks;
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type HoldIdentifier = ();
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct UniquesHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_uniques::BenchmarkHelper<MultiLocation, AssetInstance> for UniquesHelper {
	fn collection(i: u16) -> MultiLocation {
		GeneralIndex(i as u128).into()
	}
	fn item(i: u16) -> AssetInstance {
		AssetInstance::Index(i as u128)
	}
}

impl pallet_uniques::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CollectionId = MultiLocation;
	type ItemId = AssetInstance;
	type Currency = Balances;
	type CreateOrigin = ForeignCreators;
	type ForceOrigin = frame_system::EnsureRoot<AccountId>;
	type CollectionDeposit = frame_support::traits::ConstU128<1_000>;
	type ItemDeposit = frame_support::traits::ConstU128<1_000>;
	type MetadataDepositBase = frame_support::traits::ConstU128<1_000>;
	type AttributeDepositBase = frame_support::traits::ConstU128<1_000>;
	type DepositPerByte = frame_support::traits::ConstU128<1>;
	type StringLimit = ConstU32<64>;
	type KeyLimit = ConstU32<64>;
	type ValueLimit = ConstU32<128>;
	type Locker = ();
	type WeightInfo = ();
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = UniquesHelper;
}

// `EnsureOriginWithArg` impl for `CreateOrigin` which allows only XCM origins
// which are locations containing the class location.
pub struct ForeignCreators;
impl EnsureOriginWithArg<RuntimeOrigin, MultiLocation> for ForeignCreators {
	type Success = AccountId;

	fn try_origin(
		o: RuntimeOrigin,
		a: &MultiLocation,
	) -> sp_std::result::Result<Self::Success, RuntimeOrigin> {
		let origin_location = pallet_xcm::EnsureXcm::<Everything>::try_origin(o.clone())?;
		if !a.starts_with(&origin_location) {
			return Err(o)
		}
		SovereignAccountOf::convert(origin_location).map_err(|_| o)
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(a: &MultiLocation) -> Result<RuntimeOrigin, ()> {
		Ok(pallet_xcm::Origin::Xcm(a.clone()).into())
	}
}

parameter_types! {
	pub const ReservedXcmpWeight: Weight = Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND.saturating_div(4), 0);
	pub const ReservedDmpWeight: Weight = Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND.saturating_div(4), 0);
}

parameter_types! {
	pub const KsmLocation: MultiLocation = MultiLocation::parent();
	pub const RelayNetwork: NetworkId = NetworkId::Kusama;
	pub UniversalLocation: InteriorMultiLocation = Parachain(MsgQueue::parachain_id().into()).into();
}

pub type LocationToAccountId = (
	ParentIsPreset<AccountId>,
	SiblingParachainConvertsVia<Sibling, AccountId>,
	AccountId32Aliases<RelayNetwork, AccountId>,
	Account32Hash<(), AccountId>,
);

pub type XcmOriginToCallOrigin = (
	SovereignSignedViaLocation<LocationToAccountId, RuntimeOrigin>,
	SignedAccountId32AsNative<RelayNetwork, RuntimeOrigin>,
	XcmPassthrough<RuntimeOrigin>,
);

parameter_types! {
	pub const UnitWeightCost: Weight = Weight::from_parts(1, 1);
	pub KsmPerSecondPerByte: (AssetId, u128, u128) = (Concrete(Parent.into()), 1, 1);
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
	pub ForeignPrefix: MultiLocation = (Parent,).into();
}

pub type LocalAssetTransactor = (
	XcmCurrencyAdapter<Balances, IsConcrete<KsmLocation>, LocationToAccountId, AccountId, ()>,
	NonFungiblesAdapter<
		ForeignUniques,
		ConvertedConcreteId<MultiLocation, AssetInstance, JustTry, JustTry>,
		SovereignAccountOf,
		AccountId,
		NoChecking,
		(),
	>,
);

pub type XcmRouter = super::ParachainXcmRouter<MsgQueue>;
pub type Barrier = AllowUnpaidExecutionFrom<Everything>;

parameter_types! {
	pub NftCollectionOne: MultiAssetFilter
		= Wild(AllOf { fun: WildNonFungible, id: Concrete((Parent, GeneralIndex(1)).into()) });
	pub NftCollectionOneForRelay: (MultiAssetFilter, MultiLocation)
		= (NftCollectionOne::get(), (Parent,).into());
}
pub type TrustedTeleporters = xcm_builder::Case<NftCollectionOneForRelay>;
pub type TrustedReserves = EverythingBut<xcm_builder::Case<NftCollectionOneForRelay>>;

pub struct XcmConfig;
impl Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = XcmRouter;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = XcmOriginToCallOrigin;
	type IsReserve = (NativeAsset, TrustedReserves);
	type IsTeleporter = TrustedTeleporters;
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
	type Trader = FixedRateOfFungible<KsmPerSecondPerByte, ()>;
	type ResponseHandler = ();
	type AssetTrap = ();
	type AssetLocker = ();
	type AssetExchanger = ();
	type AssetClaims = ();
	type SubscriptionService = ();
	type PalletInstancesInfo = ();
	type FeeManager = ();
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = RuntimeCall;
	type SafeCallFilter = Everything;
}

#[frame_support::pallet]
pub mod mock_msg_queue {
	use super::*;
	use frame_support::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		type XcmExecutor: ExecuteXcm<Self::RuntimeCall>;
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {}

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::storage]
	#[pallet::getter(fn parachain_id)]
	pub(super) type ParachainId<T: Config> = StorageValue<_, ParaId, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn received_dmp)]
	/// A queue of received DMP messages
	pub(super) type ReceivedDmp<T: Config> = StorageValue<_, Vec<Xcm<T::RuntimeCall>>, ValueQuery>;

	impl<T: Config> Get<ParaId> for Pallet<T> {
		fn get() -> ParaId {
			Self::parachain_id()
		}
	}

	pub type MessageId = [u8; 32];

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		// XCMP
		/// Some XCM was executed OK.
		Success(Option<T::Hash>),
		/// Some XCM failed.
		Fail(Option<T::Hash>, XcmError),
		/// Bad XCM version used.
		BadVersion(Option<T::Hash>),
		/// Bad XCM format used.
		BadFormat(Option<T::Hash>),

		// DMP
		/// Downward message is invalid XCM.
		InvalidFormat(MessageId),
		/// Downward message is unsupported version of XCM.
		UnsupportedVersion(MessageId),
		/// Downward message executed with the given outcome.
		ExecutedDownward(MessageId, Outcome),
	}

	impl<T: Config> Pallet<T> {
		pub fn set_para_id(para_id: ParaId) {
			ParachainId::<T>::put(para_id);
		}

		fn handle_xcmp_message(
			sender: ParaId,
			_sent_at: RelayBlockNumber,
			xcm: VersionedXcm<T::RuntimeCall>,
			max_weight: Weight,
		) -> Result<Weight, XcmError> {
			let hash = Encode::using_encoded(&xcm, T::Hashing::hash);
			let message_hash = Encode::using_encoded(&xcm, sp_io::hashing::blake2_256);
			let (result, event) = match Xcm::<T::RuntimeCall>::try_from(xcm) {
				Ok(xcm) => {
					let location = (Parent, Parachain(sender.into()));
					match T::XcmExecutor::execute_xcm(location, xcm, message_hash, max_weight) {
						Outcome::Error(e) => (Err(e.clone()), Event::Fail(Some(hash), e)),
						Outcome::Complete(w) => (Ok(w), Event::Success(Some(hash))),
						// As far as the caller is concerned, this was dispatched without error, so
						// we just report the weight used.
						Outcome::Incomplete(w, e) => (Ok(w), Event::Fail(Some(hash), e)),
					}
				},
				Err(()) => (Err(XcmError::UnhandledXcmVersion), Event::BadVersion(Some(hash))),
			};
			Self::deposit_event(event);
			result
		}
	}

	impl<T: Config> XcmpMessageHandler for Pallet<T> {
		fn handle_xcmp_messages<'a, I: Iterator<Item = (ParaId, RelayBlockNumber, &'a [u8])>>(
			iter: I,
			max_weight: Weight,
		) -> Weight {
			for (sender, sent_at, data) in iter {
				let mut data_ref = data;
				let _ = XcmpMessageFormat::decode(&mut data_ref)
					.expect("Simulator encodes with versioned xcm format; qed");

				let mut remaining_fragments = &data_ref[..];
				while !remaining_fragments.is_empty() {
					if let Ok(xcm) =
						VersionedXcm::<T::RuntimeCall>::decode(&mut remaining_fragments)
					{
						let _ = Self::handle_xcmp_message(sender, sent_at, xcm, max_weight);
					} else {
						debug_assert!(false, "Invalid incoming XCMP message data");
					}
				}
			}
			max_weight
		}
	}

	impl<T: Config> DmpMessageHandler for Pallet<T> {
		fn handle_dmp_messages(
			iter: impl Iterator<Item = (RelayBlockNumber, Vec<u8>)>,
			limit: Weight,
		) -> Weight {
			for (_i, (_sent_at, data)) in iter.enumerate() {
				let id = sp_io::hashing::blake2_256(&data[..]);
				let maybe_versioned = VersionedXcm::<T::RuntimeCall>::decode(&mut &data[..]);
				match maybe_versioned {
					Err(_) => {
						Self::deposit_event(Event::InvalidFormat(id));
					},
					Ok(versioned) => match Xcm::try_from(versioned) {
						Err(()) => Self::deposit_event(Event::UnsupportedVersion(id)),
						Ok(x) => {
							let outcome = T::XcmExecutor::execute_xcm(Parent, x.clone(), id, limit);
							<ReceivedDmp<T>>::append(x);
							Self::deposit_event(Event::ExecutedDownward(id, outcome));
						},
					},
				}
			}
			limit
		}
	}
}

impl mock_msg_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
}

pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>;

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub ReachableDest: Option<MultiLocation> = Some(Parent.into());
}

impl pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Nothing;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = Balances;
	type CurrencyMatcher = ();
	type TrustedLockers = ();
	type SovereignAccountOf = LocationToAccountId;
	type MaxLockers = ConstU32<8>;
	type WeightInfo = pallet_xcm::TestWeightInfo;
	#[cfg(feature = "runtime-benchmarks")]
	type ReachableDest = ReachableDest;
	type AdminOrigin = EnsureRoot<AccountId>;
}

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
		MsgQueue: mock_msg_queue::{Pallet, Storage, Event<T>},
		PolkadotXcm: pallet_xcm::{Pallet, Call, Event<T>, Origin},
		ForeignUniques: pallet_uniques::{Pallet, Call, Storage, Event<T>},
	}
);
