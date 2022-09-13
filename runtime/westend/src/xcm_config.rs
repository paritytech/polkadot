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

//! XCM configurations for Westend.

use super::{
	parachains_origin, weights, AccountId, Balances, Origin, ParaId, Runtime, RuntimeCall,
	RuntimeEvent, WeightToFee, XcmPallet,
};
use frame_support::{
	parameter_types,
	traits::{Everything, Nothing},
};
use runtime_common::{xcm_sender, ToAuthor};
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowKnownQueryResponses, AllowSubscriptionsFrom,
	AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom, ChildParachainAsNative,
	ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, IsChildSystemParachain, IsConcrete, LocationInverter,
	SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation, TakeWeightCredit,
	UsingComponents, WeightInfoBounds,
};

parameter_types! {
	pub const WndLocation: MultiLocation = Here.into();
	pub const Ancestry: MultiLocation = Here.into();
	pub WestendNetwork: NetworkId =
		NetworkId::Named(b"Westend".to_vec().try_into().expect("shorter than length limit; qed"));
	pub CheckAccount: AccountId = XcmPallet::check_account();
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
	ChildParachainAsNative<parachains_origin::Origin, Origin>,
	SignedAccountId32AsNative<WestendNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

/// The XCM router. When we want to send an XCM message, we use this type. It amalgamates all of our
/// individual routers.
pub type XcmRouter = (
	// Only one router so far - use DMP to communicate with child parachains.
	xcm_sender::ChildParachainRouter<Runtime, XcmPallet>,
);

parameter_types! {
	pub const Westmint: MultiLocation = Parachain(1000).into();
	pub const Collectives: MultiLocation = Parachain(1001).into();
	pub const WestendForWestmint: (MultiAssetFilter, MultiLocation) =
		(Wild(AllOf { fun: WildFungible, id: Concrete(WndLocation::get()) }), Westmint::get());
	pub const WestendForCollectives: (MultiAssetFilter, MultiLocation) =
		(Wild(AllOf { fun: WildFungible, id: Concrete(WndLocation::get()) }), Collectives::get());
	pub const MaxInstructions: u32 = 100;
}
pub type TrustedTeleporters =
	(xcm_builder::Case<WestendForWestmint>, xcm_builder::Case<WestendForCollectives>);

/// The barriers one of which must be passed for an XCM message to be executed.
pub type Barrier = (
	// Weight that is paid for may be consumed.
	TakeWeightCredit,
	// If the message is one that immediately attemps to pay for execution, then allow it.
	AllowTopLevelPaidExecutionFrom<Everything>,
	// Messages coming from system parachains need not pay for execution.
	AllowUnpaidExecutionFrom<IsChildSystemParachain<ParaId>>,
	// Expected responses are OK.
	AllowKnownQueryResponses<XcmPallet>,
	// Subscriptions for version tracking are OK.
	AllowSubscriptionsFrom<Everything>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = XcmRouter;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type LocationInverter = LocationInverter<Ancestry>;
	type Barrier = Barrier;
	type Weigher =
		WeightInfoBounds<weights::xcm::WestendXcmWeight<RuntimeCall>, RuntimeCall, MaxInstructions>;
	type Trader = UsingComponents<WeightToFee, WndLocation, AccountId, Balances, ToAuthor<Runtime>>;
	type ResponseHandler = XcmPallet;
	type AssetTrap = XcmPallet;
	type AssetClaims = XcmPallet;
	type SubscriptionService = XcmPallet;
}

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<Origin, AccountId, WestendNetwork>,
);

impl pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	// ...but they must match our filter, which rejects everything.
	type XcmExecuteFilter = Nothing;
	type XcmExecutor = xcm_executor::XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher =
		WeightInfoBounds<weights::xcm::WestendXcmWeight<RuntimeCall>, RuntimeCall, MaxInstructions>;
	type LocationInverter = LocationInverter<Ancestry>;
	type Origin = Origin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
}
