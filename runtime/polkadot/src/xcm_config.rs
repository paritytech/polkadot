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

//! XCM configuration for Polkadot.

use super::{
	parachains_origin, AccountId, AllPalletsWithSystem, Balances, Call, CouncilCollective, Event,
	Origin, ParaId, Runtime, WeightToFee, XcmPallet,
};
use frame_support::{
	match_types, parameter_types,
	traits::{Everything, Nothing},
	weights::Weight,
};
use runtime_common::{xcm_sender, ToAuthor};
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowKnownQueryResponses, AllowSubscriptionsFrom,
	AllowTopLevelPaidExecutionFrom, BackingToPlurality, ChildParachainAsNative,
	ChildParachainConvertsVia, CurrencyAdapter as XcmCurrencyAdapter, FixedWeightBounds,
	IsConcrete, SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation,
	TakeWeightCredit, UsingComponents,
};

parameter_types! {
	/// The location of the DOT token, from the context of this chain. Since this token is native to this
	/// chain, we make it synonymous with it and thus it is the `Here` location, which means "equivalent to
	/// the context".
	pub const TokenLocation: MultiLocation = Here.into_location();
	/// The Polkadot network ID. This is named.
	pub const ThisNetwork: NetworkId = NetworkId::Polkadot;
	/// Our location in the universe of consensus systems.
	pub const UniversalLocation: InteriorMultiLocation = X1(GlobalConsensus(ThisNetwork::get()));
	/// The check account, which holds any native assets that have been teleported out and not back in (yet).
	pub CheckAccount: AccountId = XcmPallet::check_account();
}

/// The canonical means of converting a `MultiLocation` into an `AccountId`, used when we want to determine
/// the sovereign account controlled by a location.
pub type SovereignAccountOf = (
	// We can convert a child parachain using the standard `AccountId` conversion.
	ChildParachainConvertsVia<ParaId, AccountId>,
	// We can directly alias an `AccountId32` into a local account.
	AccountId32Aliases<ThisNetwork, AccountId>,
);

/// Our asset transactor. This is what allows us to interact with the runtime assets from the point of
/// view of XCM-only concepts like `MultiLocation` and `MultiAsset`.
///
/// Ours is only aware of the Balances pallet, which is mapped to `TokenLocation`.
pub type LocalAssetTransactor = XcmCurrencyAdapter<
	// Use this currency:
	Balances,
	// Use this currency when it is a fungible asset matching the given location or name:
	IsConcrete<TokenLocation>,
	// We can convert the MultiLocations with our converter above:
	SovereignAccountOf,
	// Our chain's account ID type (we can't get away without mentioning it explicitly):
	AccountId,
	// We track our teleports in/out to keep total issuance correct.
	CheckAccount,
>;

/// The means that we convert an XCM origin `MultiLocation` into the runtime's `Origin` type for
/// local dispatch. This is a conversion function from an `OriginKind` type along with the
/// `MultiLocation` value and returns an `Origin` value or an error.
type LocalOriginConverter = (
	// If the origin kind is `Sovereign`, then return a `Signed` origin with the account determined
	// by the `SovereignAccountOf` converter.
	SovereignSignedViaLocation<SovereignAccountOf, Origin>,
	// If the origin kind is `Native` and the XCM origin is a child parachain, then we can express
	// it with the special `parachains_origin::Origin` origin variant.
	ChildParachainAsNative<parachains_origin::Origin, Origin>,
	// If the origin kind is `Native` and the XCM origin is the `AccountId32` location, then it can
	// be expressed using the `Signed` origin variant.
	SignedAccountId32AsNative<ThisNetwork, Origin>,
);

parameter_types! {
	/// The amount of weight an XCM operation takes. This is a safe overestimate.
	pub const BaseXcmWeight: Weight = 1_000_000_000;
	/// Maximum number of instructions in a single XCM fragment. A sanity check against weight
	/// calculations getting too crazy.
	pub const MaxInstructions: u32 = 100;
}

/// The XCM router. When we want to send an XCM message, we use this type. It amalgamates all of our
/// individual routers.
pub type XcmRouter = (
	// Only one router so far - use DMP to communicate with child parachains.
	xcm_sender::ChildParachainRouter<Runtime, XcmPallet, ()>,
);

parameter_types! {
	pub const Dot: MultiAssetFilter = Wild(AllOf { fun: WildFungible, id: Concrete(TokenLocation::get()) });
	pub const DotForStatemint: (MultiAssetFilter, MultiLocation) = (Dot::get(), Parachain(1000).into_location());
	pub const MaxAssetsIntoHolding: u32 = 64;
}

/// Polkadot Relay recognizes/respects the Statemint chain as a teleporter.
pub type TrustedTeleporters = (xcm_builder::Case<DotForStatemint>,);

match_types! {
	pub type OnlyParachains: impl Contains<MultiLocation> = {
		MultiLocation { parents: 0, interior: X1(Parachain(_)) }
	};
}

/// The barriers one of which must be passed for an XCM message to be executed.
pub type Barrier = (
	// Weight that is paid for may be consumed.
	TakeWeightCredit,
	// If the message is one that immediately attemps to pay for execution, then allow it.
	AllowTopLevelPaidExecutionFrom<Everything>,
	// Expected responses are OK.
	AllowKnownQueryResponses<XcmPallet>,
	// Subscriptions for version tracking are OK.
	AllowSubscriptionsFrom<OnlyParachains>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = XcmRouter;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	// Polkadot Relay recognises no chains which act as reserves.
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call, MaxInstructions>;
	// The weight trader piggybacks on the existing transaction-fee conversion logic.
	type Trader =
		UsingComponents<WeightToFee, TokenLocation, AccountId, Balances, ToAuthor<Runtime>>;
	type ResponseHandler = XcmPallet;
	type AssetTrap = XcmPallet;
	type AssetLocker = ();
	type AssetExchanger = ();
	type AssetClaims = XcmPallet;
	type SubscriptionService = XcmPallet;
	type PalletInstancesInfo = AllPalletsWithSystem;
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type FeeManager = ();
	// No bridges yet...
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = Call;
}

parameter_types! {
	pub const CouncilBodyId: BodyId = BodyId::Executive;
}

/// Type to convert a council origin to a Plurality `MultiLocation` value.
pub type CouncilToPlurality = BackingToPlurality<
	Origin,
	pallet_collective::Origin<Runtime, CouncilCollective>,
	CouncilBodyId,
>;

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// We allow an origin from the Collective pallet to be used in XCM as a corresponding Plurality of the
	// `Unit` body.
	CouncilToPlurality,
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<Origin, AccountId, ThisNetwork>,
);

impl pallet_xcm::Config for Runtime {
	type Event = Event;
	// Only allow the council to send messages.
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, CouncilToPlurality>;
	type XcmRouter = XcmRouter;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	// ...but they must match our filter, which rejects all.
	type XcmExecuteFilter = Nothing; // == Deny All
	type XcmExecutor = xcm_executor::XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything; // == Allow All
	type XcmReserveTransferFilter = Everything; // == Allow All
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type Origin = Origin;
	type Call = Call;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = Balances;
	type CurrencyMatcher = ();
	type TrustedLockers = ();
	type SovereignAccountOf = SovereignAccountOf;
	type MaxLockers = frame_support::traits::ConstU32<8>;
}
