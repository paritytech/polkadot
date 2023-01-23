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
	parachains_origin, weights, AccountId, AllPalletsWithSystem, Balances, ParaId, Runtime,
	RuntimeCall, RuntimeEvent, RuntimeOrigin, WeightToFee, XcmPallet,
};
use frame_support::{
	parameter_types,
	traits::{Contains, Everything, Nothing},
};
use runtime_common::{xcm_sender, ToAuthor};
use sp_core::ConstU32;
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowExplicitUnpaidExecutionFrom, AllowKnownQueryResponses,
	AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom, ChildParachainAsNative,
	ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, IsChildSystemParachain, IsConcrete, MintLocation,
	SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation, TakeWeightCredit,
	UsingComponents, WeightInfoBounds, WithComputedOrigin,
};
use xcm_executor::{traits::WithOriginFilter, XcmExecutor};

parameter_types! {
	pub const TokenLocation: MultiLocation = Here.into_location();
	pub const ThisNetwork: NetworkId = Westend;
	pub UniversalLocation: InteriorMultiLocation = ThisNetwork::get().into();
	pub CheckAccount: AccountId = XcmPallet::check_account();
	pub LocalCheckAccount: (AccountId, MintLocation) = (CheckAccount::get(), MintLocation::Local);
}

pub type LocationConverter =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<ThisNetwork, AccountId>);

pub type LocalAssetTransactor = XcmCurrencyAdapter<
	// Use this currency:
	Balances,
	// Use this currency when it is a fungible asset matching the given location or name:
	IsConcrete<TokenLocation>,
	// We can convert the MultiLocations with our converter above:
	LocationConverter,
	// Our chain's account ID type (we can't get away without mentioning it explicitly):
	AccountId,
	// It's a native asset so we keep track of the teleports to maintain total issuance.
	LocalCheckAccount,
>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<LocationConverter, RuntimeOrigin>,
	ChildParachainAsNative<parachains_origin::Origin, RuntimeOrigin>,
	SignedAccountId32AsNative<ThisNetwork, RuntimeOrigin>,
	ChildSystemParachainAsSuperuser<ParaId, RuntimeOrigin>,
);

/// The XCM router. When we want to send an XCM message, we use this type. It amalgamates all of our
/// individual routers.
pub type XcmRouter = (
	// Only one router so far - use DMP to communicate with child parachains.
	xcm_sender::ChildParachainRouter<Runtime, XcmPallet, ()>,
);

parameter_types! {
	pub const Westmint: MultiLocation = Parachain(1000).into_location();
	pub const Collectives: MultiLocation = Parachain(1001).into_location();
	pub const Wnd: MultiAssetFilter = Wild(AllOf { fun: WildFungible, id: Concrete(TokenLocation::get()) });
	pub const WndForWestmint: (MultiAssetFilter, MultiLocation) = (Wnd::get(), Westmint::get());
	pub const WndForCollectives: (MultiAssetFilter, MultiLocation) = (Wnd::get(), Collectives::get());
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
}

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub ReachableDest: Option<MultiLocation> = Some(Parachain(1000).into());
}

pub type TrustedTeleporters =
	(xcm_builder::Case<WndForWestmint>, xcm_builder::Case<WndForCollectives>);

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
			AllowSubscriptionsFrom<Everything>,
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
			RuntimeCall::Staking(
				pallet_staking::Call::bond { .. } |
				pallet_staking::Call::bond_extra { .. } |
				pallet_staking::Call::unbond { .. } |
				pallet_staking::Call::withdraw_unbonded { .. } |
				pallet_staking::Call::validate { .. } |
				pallet_staking::Call::nominate { .. } |
				pallet_staking::Call::chill { .. } |
				pallet_staking::Call::set_payee { .. } |
				pallet_staking::Call::set_controller { .. } |
				pallet_staking::Call::set_validator_count { .. } |
				pallet_staking::Call::increase_validator_count { .. } |
				pallet_staking::Call::scale_validator_count { .. } |
				pallet_staking::Call::force_no_eras { .. } |
				pallet_staking::Call::force_new_era { .. } |
				pallet_staking::Call::set_invulnerables { .. } |
				pallet_staking::Call::force_unstake { .. } |
				pallet_staking::Call::force_new_era_always { .. } |
				pallet_staking::Call::payout_stakers { .. } |
				pallet_staking::Call::rebond { .. } |
				pallet_staking::Call::reap_stash { .. } |
				pallet_staking::Call::set_staking_configs { .. } |
				pallet_staking::Call::chill_other { .. } |
				pallet_staking::Call::force_apply_min_commission { .. },
			) |
			RuntimeCall::Session(pallet_session::Call::purge_keys { .. }) |
			RuntimeCall::Grandpa(..) |
			RuntimeCall::ImOnline(..) |
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
			RuntimeCall::Recovery(..) |
			RuntimeCall::Vesting(..) |
			RuntimeCall::ElectionProviderMultiPhase(..) |
			RuntimeCall::VoterList(..) |
			RuntimeCall::NominationPools(
				pallet_nomination_pools::Call::join { .. } |
				pallet_nomination_pools::Call::bond_extra { .. } |
				pallet_nomination_pools::Call::claim_payout { .. } |
				pallet_nomination_pools::Call::unbond { .. } |
				pallet_nomination_pools::Call::pool_withdraw_unbonded { .. } |
				pallet_nomination_pools::Call::withdraw_unbonded { .. } |
				pallet_nomination_pools::Call::create { .. } |
				pallet_nomination_pools::Call::create_with_pool_id { .. } |
				pallet_nomination_pools::Call::set_state { .. } |
				pallet_nomination_pools::Call::set_configs { .. } |
				pallet_nomination_pools::Call::update_roles { .. } |
				pallet_nomination_pools::Call::chill { .. },
			) |
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
	type Weigher =
		WeightInfoBounds<weights::xcm::WestendXcmWeight<RuntimeCall>, RuntimeCall, MaxInstructions>;
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

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<RuntimeOrigin, AccountId, ThisNetwork>,
);

impl pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	// ...but they must match our filter, which rejects everything.
	type XcmExecuteFilter = Nothing;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher =
		WeightInfoBounds<weights::xcm::WestendXcmWeight<RuntimeCall>, RuntimeCall, MaxInstructions>;
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
