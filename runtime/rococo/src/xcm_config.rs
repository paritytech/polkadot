// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! XCM configuration for Rococo.

use super::{
	parachains_origin, AccountId, Balances, Call, Event, Origin, ParaId, Runtime, WeightToFee,
	XcmPallet,
};
use frame_support::{
	parameter_types,
	traits::{Everything, IsInVec, Nothing},
};
use runtime_common::{xcm_sender, ToAuthor};
use sp_std::prelude::*;
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowKnownQueryResponses, AllowSubscriptionsFrom, BackingToPlurality,
	ChildParachainAsNative, ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedWeightBounds, IsConcrete, LocationInverter,
	SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation, UsingComponents,
};

parameter_types! {
	pub const RocLocation: MultiLocation = Here.into();
	pub const RococoNetwork: NetworkId = NetworkId::Polkadot;
	pub const Ancestry: MultiLocation = Here.into();
	pub CheckAccount: AccountId = XcmPallet::check_account();
}

pub type SovereignAccountOf =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<RococoNetwork, AccountId>);

pub type LocalAssetTransactor = XcmCurrencyAdapter<
	// Use this currency:
	Balances,
	// Use this currency when it is a fungible asset matching the given location or name:
	IsConcrete<RocLocation>,
	// We can convert the MultiLocations with our converter above:
	SovereignAccountOf,
	// Our chain's account ID type (we can't get away without mentioning it explicitly):
	AccountId,
	// It's a native asset so we keep track of the teleports to maintain total issuance.
	CheckAccount,
>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<SovereignAccountOf, Origin>,
	ChildParachainAsNative<parachains_origin::Origin, Origin>,
	SignedAccountId32AsNative<RococoNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

parameter_types! {
	pub const BaseXcmWeight: u64 = 1_000_000_000;
}

/// The XCM router. When we want to send an XCM message, we use this type. It amalgamates all of our
/// individual routers.
pub type XcmRouter = (
	// Only one router so far - use DMP to communicate with child parachains.
	xcm_sender::ChildParachainRouter<Runtime, XcmPallet>,
);

parameter_types! {
	pub const Rococo: MultiAssetFilter = Wild(AllOf { fun: WildFungible, id: Concrete(RocLocation::get()) });
	pub const RococoForTick: (MultiAssetFilter, MultiLocation) = (Rococo::get(), Parachain(100).into());
	pub const RococoForTrick: (MultiAssetFilter, MultiLocation) = (Rococo::get(), Parachain(110).into());
	pub const RococoForTrack: (MultiAssetFilter, MultiLocation) = (Rococo::get(), Parachain(120).into());
	pub const RococoForStatemine: (MultiAssetFilter, MultiLocation) = (Rococo::get(), Parachain(1000).into());
	pub const RococoForCanvas: (MultiAssetFilter, MultiLocation) = (Rococo::get(), Parachain(1002).into());
	pub const RococoForEncointer: (MultiAssetFilter, MultiLocation) = (Rococo::get(), Parachain(1003).into());
	pub const MaxInstructions: u32 = 100;
}
pub type TrustedTeleporters = (
	xcm_builder::Case<RococoForTick>,
	xcm_builder::Case<RococoForTrick>,
	xcm_builder::Case<RococoForTrack>,
	xcm_builder::Case<RococoForStatemine>,
	xcm_builder::Case<RococoForCanvas>,
	xcm_builder::Case<RococoForEncointer>,
);

parameter_types! {
	pub AllowUnpaidFrom: Vec<MultiLocation> =
		vec![
			Parachain(100).into(),
			Parachain(110).into(),
			Parachain(120).into(),
			Parachain(1000).into(),
			Parachain(1002).into(),
			Parachain(1003).into(),
		];
}

use xcm_builder::{AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom, TakeWeightCredit};
pub type Barrier = (
	TakeWeightCredit,
	AllowTopLevelPaidExecutionFrom<Everything>,
	AllowUnpaidExecutionFrom<IsInVec<AllowUnpaidFrom>>, // <- Trusted parachains get free execution
	// Expected responses are OK.
	AllowKnownQueryResponses<XcmPallet>,
	// Subscriptions for version tracking are OK.
	AllowSubscriptionsFrom<Everything>,
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
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call, MaxInstructions>;
	type Trader = UsingComponents<WeightToFee, RocLocation, AccountId, Balances, ToAuthor<Runtime>>;
	type ResponseHandler = XcmPallet;
	type AssetTrap = XcmPallet;
	type AssetClaims = XcmPallet;
	type SubscriptionService = XcmPallet;
}

parameter_types! {
	pub const CollectiveBodyId: BodyId = BodyId::Unit;
}

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// We allow an origin from the Collective pallet to be used in XCM as a corresponding Plurality of the
	// `Unit` body.
	BackingToPlurality<Origin, pallet_collective::Origin<Runtime>, CollectiveBodyId>,
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<Origin, AccountId, RococoNetwork>,
);

impl pallet_xcm::Config for Runtime {
	type Event = Event;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	// ...but they must match our filter, which right now rejects everything.
	type XcmExecuteFilter = Nothing;
	type XcmExecutor = xcm_executor::XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call, MaxInstructions>;
	type LocationInverter = LocationInverter<Ancestry>;
	type Origin = Origin;
	type Call = Call;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
}
