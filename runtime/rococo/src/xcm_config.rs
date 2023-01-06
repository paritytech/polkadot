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
	parachains_origin, AccountId, AllPalletsWithSystem, Balances, CouncilCollective, ParaId,
	Runtime, RuntimeCall, RuntimeEvent, RuntimeOrigin, WeightToFee, XcmPallet,
};
use frame_support::{
	match_types, parameter_types,
	traits::{Contains, Everything, Nothing},
	weights::Weight,
};
use runtime_common::{xcm_sender, ToAuthor};
use sp_core::ConstU32;
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowExplicitUnpaidExecutionFrom, AllowKnownQueryResponses,
	AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom, BackingToPlurality,
	ChildParachainAsNative, ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedWeightBounds, IsChildSystemParachain, IsConcrete,
	MintLocation, SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation,
	TakeWeightCredit, UsingComponents, WeightInfoBounds, WithComputedOrigin,
};
use xcm_executor::{traits::WithOriginFilter, XcmExecutor};

parameter_types! {
	pub const TokenLocation: MultiLocation = Here.into_location();
	pub const ThisNetwork: NetworkId = NetworkId::Rococo;
	pub UniversalLocation: InteriorMultiLocation = ThisNetwork::get().into();
	pub CheckAccount: AccountId = XcmPallet::check_account();
	pub LocalCheckAccount: (AccountId, MintLocation) = (CheckAccount::get(), MintLocation::Local);
}

pub type LocationConverter =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<ThisNetwork, AccountId>);

/// Our asset transactor. This is what allows us to interest with the runtime facilities from the point of
/// view of XCM-only concepts like `MultiLocation` and `MultiAsset`.
///
/// Ours is only aware of the Balances pallet, which is mapped to `RocLocation`.
pub type LocalAssetTransactor = XcmCurrencyAdapter<
	// Use this currency:
	Balances,
	// Use this currency when it is a fungible asset matching the given location or name:
	IsConcrete<TokenLocation>,
	// We can convert the MultiLocations with our converter above:
	LocationConverter,
	// Our chain's account ID type (we can't get away without mentioning it explicitly):
	AccountId,
	// We track our teleports in/out to keep total issuance correct.
	LocalCheckAccount,
>;

/// The means that we convert an the XCM message origin location into a local dispatch origin.
type LocalOriginConverter = (
	// A `Signed` origin of the sovereign account that the original location controls.
	SovereignSignedViaLocation<LocationConverter, RuntimeOrigin>,
	// A child parachain, natively expressed, has the `Parachain` origin.
	ChildParachainAsNative<parachains_origin::Origin, RuntimeOrigin>,
	// The AccountId32 location type can be expressed natively as a `Signed` origin.
	SignedAccountId32AsNative<ThisNetwork, RuntimeOrigin>,
	// A system child parachain, expressed as a Superuser, converts to the `Root` origin.
	ChildSystemParachainAsSuperuser<ParaId, RuntimeOrigin>,
);

parameter_types! {
	/// The amount of weight an XCM operation takes. This is a safe overestimate.
	pub const BaseXcmWeight: Weight = Weight::from_parts(1_000_000_000, 64 * 1024);
}
/// The XCM router. When we want to send an XCM message, we use this type. It amalgamates all of our
/// individual routers.
pub type XcmRouter = (
	// Only one router so far - use DMP to communicate with child parachains.
	xcm_sender::ChildParachainRouter<Runtime, XcmPallet, ()>,
);

parameter_types! {
	pub const Roc: MultiAssetFilter = Wild(AllOf { fun: WildFungible, id: Concrete(TokenLocation::get()) });
	pub const Statemine: MultiLocation = Parachain(1000).into_location();
	pub const Contracts: MultiLocation = Parachain(1002).into_location();
	pub const Encointer: MultiLocation = Parachain(1003).into_location();
	pub const Tick: MultiLocation = Parachain(100).into_location();
	pub const Trick: MultiLocation = Parachain(110).into_location();
	pub const Track: MultiLocation = Parachain(120).into_location();
	pub const RocForTick: (MultiAssetFilter, MultiLocation) = (Roc::get(), Tick::get());
	pub const RocForTrick: (MultiAssetFilter, MultiLocation) = (Roc::get(), Trick::get());
	pub const RocForTrack: (MultiAssetFilter, MultiLocation) = (Roc::get(), Track::get());
	pub const RocForStatemine: (MultiAssetFilter, MultiLocation) = (Roc::get(), Statemine::get());
	pub const RocForContracts: (MultiAssetFilter, MultiLocation) = (Roc::get(), Contracts::get());
	pub const RocForEncointer: (MultiAssetFilter, MultiLocation) = (Roc::get(), Encointer::get());
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
}
pub type TrustedTeleporters = (
	xcm_builder::Case<RocForTick>,
	xcm_builder::Case<RocForTrick>,
	xcm_builder::Case<RocForTrack>,
	xcm_builder::Case<RocForStatemine>,
	xcm_builder::Case<RocForContracts>,
	xcm_builder::Case<RocForEncointer>,
);

match_types! {
	pub type OnlyParachains: impl Contains<MultiLocation> = {
		MultiLocation { parents: 0, interior: X1(Parachain(_)) }
	};
}

/// The barriers one of which must be passed for an XCM message to be executed.
pub type Barrier = (
	// Weight that is paid for may be consumed.
	TakeWeightCredit,
	// Expected responses are OK.
	AllowKnownQueryResponses<XcmPallet>,
	WithComputedOrigin<
		(
			// If the message is one that immediately attemps to pay for execution, then allow it.
			AllowTopLevelPaidExecutionFrom<Everything>,
			// Messages coming from system parachains need not pay for execution.
			AllowExplicitUnpaidExecutionFrom<IsChildSystemParachain<ParaId>>,
			// Subscriptions for version tracking are OK.
			AllowSubscriptionsFrom<OnlyParachains>,
		),
		UniversalLocation,
		ConstU32<8>,
	>,
);

/// A call filter for the XCM Transact instruction. This is a temporary measure until we
/// properly account for proof size weights.
///
/// Calls that are allowed through this filter must:
/// 1. Have a fixed weight;
/// 2. Cannot lead to another call being made;
/// 3. Have a defined proof size weight, e.g. no unbounded vecs in call parameters.
pub struct SafeCallFilter;
impl Contains<RuntimeCall> for SafeCallFilter {
	fn contains(call: &RuntimeCall) -> bool {
		#[cfg(feature = "runtime-benchmarks")]
		{
			if matches!(call, RuntimeCall::System(frame_system::Call::remark_with_event { .. })) {
				return true
			}
		}

		match call {
			RuntimeCall::System(
				frame_system::Call::kill_prefix { .. } | frame_system::Call::set_heap_pages { .. },
			) |
			RuntimeCall::Babe(..) |
			RuntimeCall::Timestamp(..) |
			RuntimeCall::Indices(..) |
			RuntimeCall::Balances(..) |
			RuntimeCall::Session(pallet_session::Call::purge_keys { .. }) |
			RuntimeCall::Grandpa(..) |
			RuntimeCall::ImOnline(..) |
			RuntimeCall::Democracy(
				pallet_democracy::Call::second { .. } |
				pallet_democracy::Call::vote { .. } |
				pallet_democracy::Call::emergency_cancel { .. } |
				pallet_democracy::Call::fast_track { .. } |
				pallet_democracy::Call::veto_external { .. } |
				pallet_democracy::Call::cancel_referendum { .. } |
				pallet_democracy::Call::delegate { .. } |
				pallet_democracy::Call::undelegate { .. } |
				pallet_democracy::Call::clear_public_proposals { .. } |
				pallet_democracy::Call::unlock { .. } |
				pallet_democracy::Call::remove_vote { .. } |
				pallet_democracy::Call::remove_other_vote { .. } |
				pallet_democracy::Call::blacklist { .. } |
				pallet_democracy::Call::cancel_proposal { .. },
			) |
			RuntimeCall::Council(
				pallet_collective::Call::vote { .. } |
				pallet_collective::Call::close_old_weight { .. } |
				pallet_collective::Call::disapprove_proposal { .. } |
				pallet_collective::Call::close { .. },
			) |
			RuntimeCall::TechnicalCommittee(
				pallet_collective::Call::vote { .. } |
				pallet_collective::Call::close_old_weight { .. } |
				pallet_collective::Call::disapprove_proposal { .. } |
				pallet_collective::Call::close { .. },
			) |
			RuntimeCall::PhragmenElection(
				pallet_elections_phragmen::Call::remove_voter { .. } |
				pallet_elections_phragmen::Call::submit_candidacy { .. } |
				pallet_elections_phragmen::Call::renounce_candidacy { .. } |
				pallet_elections_phragmen::Call::remove_member { .. } |
				pallet_elections_phragmen::Call::clean_defunct_voters { .. },
			) |
			RuntimeCall::TechnicalMembership(
				pallet_membership::Call::add_member { .. } |
				pallet_membership::Call::remove_member { .. } |
				pallet_membership::Call::swap_member { .. } |
				pallet_membership::Call::change_key { .. } |
				pallet_membership::Call::set_prime { .. } |
				pallet_membership::Call::clear_prime { .. },
			) |
			RuntimeCall::Treasury(..) |
			RuntimeCall::Claims(
				super::claims::Call::claim { .. } |
				super::claims::Call::mint_claim { .. } |
				super::claims::Call::move_claim { .. },
			) |
			RuntimeCall::Utility(pallet_utility::Call::as_derivative { .. }) |
			RuntimeCall::Identity(
				pallet_identity::Call::add_registrar { .. } |
				pallet_identity::Call::set_identity { .. } |
				pallet_identity::Call::clear_identity { .. } |
				pallet_identity::Call::request_judgement { .. } |
				pallet_identity::Call::cancel_request { .. } |
				pallet_identity::Call::set_fee { .. } |
				pallet_identity::Call::set_account_id { .. } |
				pallet_identity::Call::set_fields { .. } |
				pallet_identity::Call::provide_judgement { .. } |
				pallet_identity::Call::kill_identity { .. } |
				pallet_identity::Call::add_sub { .. } |
				pallet_identity::Call::rename_sub { .. } |
				pallet_identity::Call::remove_sub { .. } |
				pallet_identity::Call::quit_sub { .. },
			) |
			RuntimeCall::Society(
				pallet_society::Call::bid { .. } |
				pallet_society::Call::unbid { .. } |
				pallet_society::Call::vouch { .. } |
				pallet_society::Call::unvouch { .. } |
				pallet_society::Call::vote { .. } |
				pallet_society::Call::defender_vote { .. } |
				pallet_society::Call::payout { .. } |
				pallet_society::Call::unfound { .. } |
				pallet_society::Call::judge_suspended_member { .. } |
				pallet_society::Call::judge_suspended_candidate { .. } |
				pallet_society::Call::set_max_members { .. },
			) |
			RuntimeCall::Recovery(..) |
			RuntimeCall::Vesting(..) |
			RuntimeCall::Bounties(
				pallet_bounties::Call::propose_bounty { .. } |
				pallet_bounties::Call::approve_bounty { .. } |
				pallet_bounties::Call::propose_curator { .. } |
				pallet_bounties::Call::unassign_curator { .. } |
				pallet_bounties::Call::accept_curator { .. } |
				pallet_bounties::Call::award_bounty { .. } |
				pallet_bounties::Call::claim_bounty { .. } |
				pallet_bounties::Call::close_bounty { .. },
			) |
			RuntimeCall::ChildBounties(..) |
			RuntimeCall::XcmPallet(pallet_xcm::Call::limited_reserve_transfer_assets {
				..
			}) => true,
			_ => false,
		}
	}
}

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = XcmRouter;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = WeightInfoBounds<
		crate::weights::xcm::RococoXcmWeight<RuntimeCall>,
		RuntimeCall,
		MaxInstructions,
	>;
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
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = WithOriginFilter<SafeCallFilter>;
	type SafeCallFilter = SafeCallFilter;
}

parameter_types! {
	pub const CollectiveBodyId: BodyId = BodyId::Unit;
}

parameter_types! {
	pub const CouncilBodyId: BodyId = BodyId::Executive;
}

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub ReachableDest: Option<MultiLocation> = Some(Parachain(1000).into());
}

/// Type to convert the council origin to a Plurality `MultiLocation` value.
pub type CouncilToPlurality = BackingToPlurality<
	RuntimeOrigin,
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
	SignedToAccountId32<RuntimeOrigin, AccountId, ThisNetwork>,
);
impl pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	// Anyone can execute XCM messages locally.
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	// Anyone is able to use reserve transfers regardless of who they are and what they want to
	// transfer.
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = Balances;
	type CurrencyMatcher = IsConcrete<TokenLocation>;
	type TrustedLockers = ();
	type SovereignAccountOf = LocationConverter;
	type MaxLockers = ConstU32<8>;
	type WeightInfo = crate::weights::pallet_xcm::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type ReachableDest = ReachableDest;
}
