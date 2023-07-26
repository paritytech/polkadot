use super::*;

use frame_support::{
	macro_magic::use_attr,
	traits::{AsEnsureOriginWithArg, Nothing},
};

#[use_attr]
use frame_support::derive_impl;

use frame_support::{
	assert_ok, construct_runtime, parameter_types,
	traits::{tokens::ConvertRank, ConstU32, Everything, RankedMembers},
};
use frame_system::{EnsureRoot, EnsureSigned};
use polkadot_test_runtime::SignedExtra;
use primitives::{AccountIndex, BlakeTwo256, Signature};
use sp_runtime::{
	generic,
	traits::{Convert, ConvertToValue, MaybeEquivalence},
	AccountId32, DispatchResult,
};
use xcm_executor::{traits::ConvertLocation, XcmExecutor};

pub type Address = sp_runtime::MultiAddress<AccountId, AccountIndex>;
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

pub type BlockNumber = u32;
pub type AccountId = AccountId32;

construct_runtime!(
	pub struct Test {
		System: frame_system,
		Balances: pallet_balances,
		Assets: pallet_assets,
		Salary: pallet_salary,
		XcmPallet: pallet_xcm,
	}
);

parameter_types! {
	pub const BlockHashCount: BlockNumber = 250;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig as frame_system::DefaultConfig)]
impl frame_system::Config for Test {
	type Block = generic::Block<Header, UncheckedExtrinsic>;
	type BlockHashCount = BlockHashCount;
	type BaseCallFilter = frame_support::traits::Everything;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type PalletInfo = PalletInfo;
	type OnSetCode = ();
	type AccountData = pallet_balances::AccountData<Balance>;
	type AccountId = AccountId;
	type Lookup = sp_runtime::traits::IdentityLookup<AccountId>;
}

pub type Balance = u128;

parameter_types! {
	pub const ExistentialDeposit: Balance = 1;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ConstU32<0>;
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	pub const AssetDeposit: u128 = 1_000_000;
	pub const MetadataDepositBase: u128 = 1_000_000;
	pub const MetadataDepositPerByte: u128 = 100_000;
	pub const AssetAccountDeposit: u128 = 1_000_000;
	pub const ApprovalDeposit: u128 = 1_000_000;
	pub const AssetsStringLimit: u32 = 50;
	pub const RemoveItemsLimit: u32 = 50;
}

impl pallet_assets::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type Balance = Balance;
	type AssetId = AssetIdForAssets;
	type Currency = Balances;
	type CreateOrigin = AsEnsureOriginWithArg<EnsureSigned<AccountId>>;
	type ForceOrigin = EnsureRoot<AccountId>;
	type AssetDeposit = AssetDeposit;
	type MetadataDepositBase = MetadataDepositBase;
	type MetadataDepositPerByte = MetadataDepositPerByte;
	type AssetAccountDeposit = AssetAccountDeposit;
	type ApprovalDeposit = ApprovalDeposit;
	type StringLimit = AssetsStringLimit;
	type Freezer = ();
	type Extra = ();
	type WeightInfo = ();
	type RemoveItemsLimit = RemoveItemsLimit;
	type AssetIdParameter = AssetIdForAssets;
	type CallbackHandle = ();
}

parameter_types! {
	pub const RelayLocation: MultiLocation = Here.into_location();
	pub const AnyNetwork: Option<NetworkId> = None;
	pub UniversalLocation: InteriorMultiLocation = (ByGenesis([0; 32]), Parachain(42)).into();
	pub UnitWeightCost: u64 = 1_000;
	pub static AdvertisedXcmVersion: u32 = 3;
	pub const BaseXcmWeight: Weight = Weight::from_parts(1_000, 1_000);
	pub CurrencyPerSecondPerByte: (AssetId, u128, u128) = (Concrete(RelayLocation::get()), 1, 1);
	pub TrustedAssets: (MultiAssetFilter, MultiLocation) = (All.into(), Here.into());
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
	pub CheckingAccount: AccountId = XcmPallet::check_account();
}

type AssetIdForAssets = u128;

pub struct FromMultiLocationToAsset<MultiLocation, AssetId>(
	core::marker::PhantomData<(MultiLocation, AssetId)>,
);
impl MaybeEquivalence<MultiLocation, AssetIdForAssets>
	for FromMultiLocationToAsset<MultiLocation, AssetIdForAssets>
{
	fn convert(value: &MultiLocation) -> Option<AssetIdForAssets> {
		match value {
			MultiLocation { parents: 0, interior: Here } => Some(0 as AssetIdForAssets),
			MultiLocation { parents: 0, interior: X2(PalletInstance(1), GeneralIndex(index)) } =>
				Some(*index as AssetIdForAssets),
			_ => None,
		}
	}

	fn convert_back(value: &AssetIdForAssets) -> Option<MultiLocation> {
		match value {
			0u128 => Some(MultiLocation { parents: 1, interior: Here }),
			para_id @ 1..=1000 =>
				Some(MultiLocation { parents: 1, interior: X1(Parachain(*para_id as u32)) }),
			_ => None,
		}
	}
}

pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, AnyNetwork>;
pub type LocalAssetsTransactor = FungiblesAdapter<
	Assets,
	ConvertedConcreteId<
		AssetIdForAssets,
		Balance,
		FromMultiLocationToAsset<MultiLocation, AssetIdForAssets>,
		JustTry,
	>,
	SovereignAccountOf,
	AccountId,
	NoChecking,
	CheckingAccount,
>;
type OriginConverter = (
	pallet_xcm::XcmPassthrough<RuntimeOrigin>,
	SignedAccountId32AsNative<AnyNetwork, RuntimeOrigin>,
);
type Barrier = AllowUnpaidExecutionFrom<Everything>;

pub struct DummyWeightTrader;
impl WeightTrader for DummyWeightTrader {
	fn new() -> Self {
		DummyWeightTrader
	}

	fn buy_weight(
		&mut self,
		_weight: Weight,
		_payment: xcm_executor::Assets,
	) -> Result<xcm_executor::Assets, XcmError> {
		Ok(xcm_executor::Assets::default())
	}
}

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = TestMessageSender;
	type AssetTransactor = LocalAssetsTransactor;
	type OriginConverter = OriginConverter;
	type IsReserve = ();
	type IsTeleporter = ();
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type Trader = DummyWeightTrader;
	type ResponseHandler = XcmPallet;
	type AssetTrap = XcmPallet;
	type AssetLocker = ();
	type AssetExchanger = ();
	type AssetClaims = XcmPallet;
	type SubscriptionService = XcmPallet;
	type PalletInstancesInfo = ();
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type FeeManager = ();
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = RuntimeCall;
	type SafeCallFilter = Everything;
	type Aliasers = Nothing;
}

parameter_types! {
	pub TreasuryAccountId: AccountId = AccountId::new([42u8; 32]);
}

pub struct TreasuryToAccount;
impl ConvertLocation<AccountId> for TreasuryToAccount {
	fn convert_location(location: &MultiLocation) -> Option<AccountId> {
		match location {
			MultiLocation {
				parents: 1,
				interior:
					X2(Parachain(42), Plurality { id: BodyId::Treasury, part: BodyPart::Voice }),
			} => Some(TreasuryAccountId::get()), // Hardcoded test treasury account id
			_ => None,
		}
	}
}

type SovereignAccountOf = (AccountId32Aliases<(), AccountId>, TreasuryToAccount);

impl pallet_xcm::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = TestMessageSender;
	type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = AdvertisedXcmVersion;
	type TrustedLockers = ();
	type SovereignAccountOf = SovereignAccountOf;
	type Currency = Balances;
	type CurrencyMatcher = IsConcrete<RelayLocation>;
	type MaxLockers = frame_support::traits::ConstU32<8>;
	type MaxRemoteLockConsumers = frame_support::traits::ConstU32<0>;
	type RemoteLockConsumerIdentifier = ();
	type WeightInfo = pallet_xcm::TestWeightInfo;
	#[cfg(feature = "runtime-benchmarks")]
	type ReachableDest = ReachableDest;
	type AdminOrigin = EnsureRoot<AccountId>;
}
