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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! New governance configurations for the Kusama runtime.

use super::*;
use frame_support::{parameter_types, traits::{EitherOf, EitherOfDiverse}};
use frame_system::EnsureRootWithSuccess;

parameter_types! {
	pub FellowshipMotionDuration: BlockNumber = prod_or_fast!(3 * DAYS, 2 * MINUTES, "KSM_MOTION_DURATION");
	pub const FellowshipMaxProposals: u32 = 100;
	pub const FellowshipMaxMembers: u32 = 200;
}

pub type FellowshipCollective = pallet_collective::Instance3;
impl pallet_collective::Config<FellowshipCollective> for Runtime {
	type Origin = Origin;
	type Proposal = Call;
	type Event = Event;
	type MotionDuration = FellowshipMotionDuration;
	type MaxProposals = FellowshipMaxProposals;
	type MaxMembers = FellowshipMaxMembers;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type WeightInfo = weights::pallet_collective_technical_committee::WeightInfo<Runtime>;
}

impl pallet_membership::Config<pallet_membership::Instance2> for Runtime {
	type Event = Event;
	type AddOrigin = FellowshipAdmin<AccountId>;
	type RemoveOrigin = FellowshipAdmin<AccountId>;
	type SwapOrigin = FellowshipAdmin<AccountId>;
	type ResetOrigin = FellowshipAdmin<AccountId>;
	type PrimeOrigin = FellowshipAdmin<AccountId>;
	type MembershipInitialized = Fellowship;
	type MembershipChanged = Fellowship;
	type MaxMembers = FellowshipMaxMembers;
	type WeightInfo = weights::pallet_membership::WeightInfo<Runtime>;
}

parameter_types! {
	pub const VoteLockingPeriod: BlockNumber = 7 * DAYS;
}

impl pallet_conviction_voting::Config for Runtime {
	type WeightInfo = pallet_conviction_voting::weights::SubstrateWeight<Self>; //TODO
	type Event = Event;
	type Currency = Balances;
	type VoteLockingPeriod = VoteLockingPeriod;
	type MaxVotes = ConstU32<512>;
	type MaxTurnout = frame_support::traits::TotalIssuanceOf<Balances, Self::AccountId>;
	type Polls = Referenda;
}

parameter_types! {
	pub const AlarmInterval: BlockNumber = 1;
	pub const SubmissionDeposit: Balance = 100 * UNITS;
	pub const UndecidingTimeout: BlockNumber = 28 * DAYS;
}

#[frame_support::pallet]
pub mod pallet_custom_origins {
	use frame_support::pallet_prelude::*;
	use super::{Balance, UNITS};

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo)]
	#[cfg_attr(feature = "std", derive(Debug))]
	#[pallet::origin]
	pub enum Origin {
		/// Origin for cancelling slashes.
		StakingAdmin,
		/// Origin for spending (any amount of) funds.
		Treasurer,
		/// Origin for managing the composition of the fellowship.
		FellowshipAdmin,
		/// Origin for managing the registrar.
		GeneralAdmin,
		/// Origin for starting auctions.
		AuctionAdmin,
		/// Origin able to force slot leases.
		LeaseAdmin,
		/// Origin able to cancel referenda.
		ReferendumCanceller,
		/// Origin able to kill referenda.
		ReferendumKiller,
		/// Origin able to spend up to 1 KSM from the treasury at once.
		SmallTipper,
		/// Origin able to spend up to 5 KSM from the treasury at once.
		BigTipper,
		/// Origin able to spend up to 50 KSM from the treasury at once.
		SmallSpender,
		/// Origin able to spend up to 500 KSM from the treasury at once.
		MediumSpender,
		/// Origin able to spend up to 5,000 KSM from the treasury at once.
		BigSpender,
		/// Origin able to dispatch a whitelisted call.
		WhitelistedCaller,
	}

	macro_rules! decl_ensure {
		( $name:ident: $success_type:ty = $success:expr ) => {
			pub struct $name<AccountId>(sp_std::marker::PhantomData<AccountId>);
			impl<O: Into<Result<Origin, O>> + From<Origin>, AccountId>
				EnsureOrigin<O> for $name<AccountId>
			{
				type Success = $success_type;
				fn try_origin(o: O) -> Result<Self::Success, O> {
					o.into().and_then(|o| match o {
						Origin::$name => Ok($success),
						r => Err(O::from(r)),
					})
				}
				#[cfg(feature = "runtime-benchmarks")]
				fn successful_origin() -> O {
					O::from(Origin::$name)
				}
			}
		};
		( $name:ident ) => { decl_ensure! { $name : () = () } };
		( $name:ident: $success_type:ty = $success:expr, $( $rest:tt )* ) => {
			decl_ensure! { $name: $success_type = $success }
			decl_ensure! { $( $rest )* }
		};
		( $name:ident, $( $rest:tt )* ) => {
			decl_ensure! { $name }
			decl_ensure! { $( $rest )* }
		};
		() => {}
	}
	decl_ensure!(
		StakingAdmin,
		Treasurer,
		FellowshipAdmin,
		GeneralAdmin,
		AuctionAdmin,
		LeaseAdmin,
		ReferendumCanceller,
		ReferendumKiller,
		SmallTipper: Balance = 250 * QUID,
		BigTipper: Balance = 1 * GRAND,
		SmallSpender: Balance = 10 * GRAND,
		MediumSpender: Balance = 100 * GRAND,
		BigSpender: Balance = 1_000 * GRAND,
		WhitelistedCaller,
	);
}
pub use pallet_custom_origins::{
	StakingAdmin, Treasurer, FellowshipAdmin, GeneralAdmin, AuctionAdmin, LeaseAdmin,
	ReferendumCanceller, ReferendumKiller, SmallTipper, BigTipper, SmallSpender, MediumSpender,
	BigSpender, WhitelistedCaller,
};

parameter_types! {
	pub const MaxBalance: Balance = Balance::max_value();
}
pub type TreasurySpender = EitherOf<
	EnsureRootWithSuccess<AccountId, MaxBalance>,
	EitherOf<
		EitherOf<SmallTipper<AccountId>, BigTipper<AccountId>>,
		EitherOf<
			SmallSpender<AccountId>,
			EitherOf<MediumSpender<AccountId>, BigSpender<AccountId>>
		>
	>
>;

impl pallet_custom_origins::Config for Runtime {}

const fn percent(x: i32) -> sp_arithmetic::FixedI64 {
	sp_arithmetic::FixedI64::from_rational(x as u128, 100)
}
use pallet_referenda::Curve;
const APP_ROOT: Curve = Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_ROOT: Curve = Curve::make_linear(28, 28, percent(0), percent(50));
const APP_STAKING_ADMIN: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_STAKING_ADMIN: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_TREASURER: Curve = Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_TREASURER: Curve = Curve::make_linear(28, 28, percent(0), percent(50));
const APP_FELLOWSHIP_ADMIN: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_FELLOWSHIP_ADMIN: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_GENERAL_ADMIN: Curve = Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_GENERAL_ADMIN: Curve = Curve::make_reciprocal(7, 28, percent(10), percent(0), percent(50));
const APP_AUCTION_ADMIN: Curve = Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_AUCTION_ADMIN: Curve = Curve::make_reciprocal(7, 28, percent(10), percent(0), percent(50));
const APP_LEASE_ADMIN: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_LEASE_ADMIN: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_REFERENDUM_CANCELLER: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_REFERENDUM_CANCELLER: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_REFERENDUM_KILLER: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_REFERENDUM_KILLER: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_SMALL_TIPPER: Curve = Curve::make_linear(10, 28, percent(50), percent(100));
const SUP_SMALL_TIPPER: Curve = Curve::make_reciprocal(1, 28, percent(4), percent(0), percent(50));
const APP_BIG_TIPPER: Curve = Curve::make_linear(10, 28, percent(50), percent(100));
const SUP_BIG_TIPPER: Curve = Curve::make_reciprocal(8, 28, percent(1), percent(0), percent(50));
const APP_SMALL_SPENDER: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_SMALL_SPENDER: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_MEDIUM_SPENDER: Curve = Curve::make_linear(23, 28, percent(50), percent(100));
const SUP_MEDIUM_SPENDER: Curve = Curve::make_reciprocal(16, 28, percent(1), percent(0), percent(50));
const APP_BIG_SPENDER: Curve = Curve::make_linear(28, 28, percent(50), percent(100));
const SUP_BIG_SPENDER: Curve = Curve::make_reciprocal(20, 28, percent(1), percent(0), percent(50));
const APP_WHITELISTED_CALLER: Curve = Curve::make_reciprocal(16, 28 * 24, percent(96), percent(50), percent(100));
const SUP_WHITELISTED_CALLER: Curve = Curve::make_reciprocal(1, 28, percent(20), percent(10), percent(50));

const TRACKS_DATA: [(u16, pallet_referenda::TrackInfo<Balance, BlockNumber>); 15] = [
	(
		0,
		pallet_referenda::TrackInfo {
			name: "root",
			max_deciding: 1,
			decision_deposit: 1_000 * GRAND,
			prepare_period: 3 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 3 * HOURS,
			min_approval: APP_ROOT,
			min_support: SUP_ROOT,
		},
	), (
		1,
		pallet_referenda::TrackInfo {
			name: "whitelisted_caller",
			max_deciding: 10,
			decision_deposit: 10_000 * GRAND,
			prepare_period: 3 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 10 * MINUTES,
			min_enactment_period: 30 * MINUTES,
			min_approval: APP_WHITELISTED_CALLER,
			min_support: SUP_WHITELISTED_CALLER,
		},
	), (
		10,
		pallet_referenda::TrackInfo {
			name: "staking_admin",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 2 * DAYS,
			min_approval: APP_STAKING_ADMIN,
			min_support: SUP_STAKING_ADMIN,
		},
	), (
		11,
		pallet_referenda::TrackInfo {
			name: "treasurer",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 2 * DAYS,
			min_approval: APP_TREASURER,
			min_support: SUP_TREASURER,
		},
	), (
		12,
		pallet_referenda::TrackInfo {
			name: "lease_admin",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 2 * DAYS,
			min_approval: APP_LEASE_ADMIN,
			min_support: SUP_LEASE_ADMIN,
		},
	), (
		13,
		pallet_referenda::TrackInfo {
			name: "fellowship_admin",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 2 * DAYS,
			min_approval: APP_FELLOWSHIP_ADMIN,
			min_support: SUP_FELLOWSHIP_ADMIN,
		},
	), (
		14,
		pallet_referenda::TrackInfo {
			name: "general_admin",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 2 * DAYS,
			min_approval: APP_GENERAL_ADMIN,
			min_support: SUP_GENERAL_ADMIN,
		},
	), (
		15,
		pallet_referenda::TrackInfo {
			name: "auction_admin",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 2 * DAYS,
			min_approval: APP_AUCTION_ADMIN,
			min_support: SUP_AUCTION_ADMIN,
		},
	), (
		20,
		pallet_referenda::TrackInfo {
			name: "referendum_canceller",
			max_deciding: 1_000,
			decision_deposit: 50 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_REFERENDUM_CANCELLER,
			min_support: SUP_REFERENDUM_CANCELLER,
		},
	), (
		21,
		pallet_referenda::TrackInfo {
			name: "referendum_killer",
			max_deciding: 1_000,
			decision_deposit: 50 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_REFERENDUM_KILLER,
			min_support: SUP_REFERENDUM_KILLER,
		},
	), (
		30,
		pallet_referenda::TrackInfo {
			name: "small_tipper",
			max_deciding: 200,
			decision_deposit: 5 * QUID,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 28 * DAYS,
			min_approval: APP_SMALL_TIPPER,
			min_support: SUP_SMALL_TIPPER,
		},
	), (
		31,
		pallet_referenda::TrackInfo {
			name: "big_tipper",
			max_deciding: 100,
			decision_deposit: 50 * QUID,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 6 * HOURS,
			min_enactment_period: 28 * DAYS,
			min_approval: APP_BIG_TIPPER,
			min_support: SUP_BIG_TIPPER,
		},
	), (
		32,
		pallet_referenda::TrackInfo {
			name: "small_spender",
			max_deciding: 50,
			decision_deposit: 500 * QUID,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 12 * HOURS,
			min_enactment_period: 28 * DAYS,
			min_approval: APP_SMALL_SPENDER,
			min_support: SUP_SMALL_SPENDER,
		},
	), (
		33,
		pallet_referenda::TrackInfo {
			name: "medium_spender",
			max_deciding: 20,
			decision_deposit: 1_500 * QUID,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 24 * HOURS,
			min_enactment_period: 28 * DAYS,
			min_approval: APP_MEDIUM_SPENDER,
			min_support: SUP_MEDIUM_SPENDER,
		},
	), (
		34,
		pallet_referenda::TrackInfo {
			name: "big_spender",
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 4,
			decision_period: 28 * DAYS,
			confirm_period: 48 * HOURS,
			min_enactment_period: 28 * DAYS,
			min_approval: APP_BIG_SPENDER,
			min_support: SUP_BIG_SPENDER,
		},
	),
];

pub struct TracksInfo;
impl pallet_referenda::TracksInfo<Balance, BlockNumber> for TracksInfo {
	type Id = u16;
	type Origin = <Origin as frame_support::traits::OriginTrait>::PalletsOrigin;
	fn tracks() -> &'static [(Self::Id, pallet_referenda::TrackInfo<Balance, BlockNumber>)] {
		&TRACKS_DATA[..]
	}
	fn track_for(id: &Self::Origin) -> Result<Self::Id, ()> {
		if let Ok(system_origin) = frame_system::RawOrigin::try_from(id.clone()) {
			match system_origin {
				frame_system::RawOrigin::Root => Ok(0),
				_ => Err(()),
			}
		} else if let Ok(custom_origin) = pallet_custom_origins::Origin::try_from(id.clone()) {
			match custom_origin {
				pallet_custom_origins::Origin::WhitelistedCaller => Ok(1),
				pallet_custom_origins::Origin::StakingAdmin => Ok(10),
				pallet_custom_origins::Origin::Treasurer => Ok(11),
				pallet_custom_origins::Origin::LeaseAdmin => Ok(12),
				pallet_custom_origins::Origin::FellowshipAdmin => Ok(13),
				pallet_custom_origins::Origin::GeneralAdmin => Ok(14),
				pallet_custom_origins::Origin::AuctionAdmin => Ok(15),
				pallet_custom_origins::Origin::ReferendumCanceller => Ok(20),
				pallet_custom_origins::Origin::ReferendumKiller => Ok(21),
				pallet_custom_origins::Origin::SmallTipper => Ok(30),
				pallet_custom_origins::Origin::BigTipper => Ok(31),
				pallet_custom_origins::Origin::SmallSpender => Ok(32),
				pallet_custom_origins::Origin::MediumSpender => Ok(33),
				pallet_custom_origins::Origin::BigSpender => Ok(34),
			}
		} else {
			Err(())
		}
	}
}

pub type WhitelistOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionAtLeast<AccountId, FellowshipCollective, 3, 5>,
>;

impl pallet_whitelist::Config for Runtime {
	type WeightInfo = pallet_whitelist::weights::SubstrateWeight<Self>; //TODO
	type Event = Event;
	type Call = Call;
	type WhitelistOrigin = WhitelistOrigin;
	type DispatchWhitelistedOrigin = WhitelistedCaller<AccountId>;
	type PreimageProvider = super::Preimage;
}

impl pallet_referenda::Config for Runtime {
	type WeightInfo = pallet_referenda::weights::SubstrateWeight<Self>; //TODO
	type Call = Call;
	type Event = Event;
	type Scheduler = Scheduler;
	type Currency = Balances;
	type CancelOrigin = ReferendumCanceller<AccountId>;
	type KillOrigin = ReferendumKiller<AccountId>;
	type Slash = ();
	type Votes = pallet_conviction_voting::VotesOf<Runtime>;
	type Tally = pallet_conviction_voting::TallyOf<Runtime>;
	type SubmissionDeposit = SubmissionDeposit;
	type MaxQueued = ConstU32<100>;
	type UndecidingTimeout = UndecidingTimeout;
	type AlarmInterval = AlarmInterval;
	type Tracks = TracksInfo;
}
