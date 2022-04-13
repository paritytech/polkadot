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

//! Old governance configurations for the Kusama runtime.

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
		SmallTipper: Balance = 1 * UNITS,
		BigTipper: Balance = 5 * UNITS,
		SmallSpender: Balance = 50 * UNITS,
		MediumSpender: Balance = 500 * UNITS,
		BigSpender: Balance = 5_000 * UNITS,
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
const TRACKS_DATA: [(u16, pallet_referenda::TrackInfo<Balance, BlockNumber>); 15] = [
	(
		0,
		pallet_referenda::TrackInfo {
			name: "root",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_reciprocal(4, 28, percent(80), percent(50)),
			min_support: Curve::make_linear(28, 28, percent(0)),
		},
	), (
		1,
		pallet_referenda::TrackInfo {
			name: "staking_admin",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(17, 28, percent(50)),
			min_support: Curve::make_reciprocal(12, 28, percent(1), percent(0)),
		},
	), (
		2,
		pallet_referenda::TrackInfo {
			name: "treasurer",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_reciprocal(4, 28, percent(80), percent(50)),
			min_support: Curve::make_linear(28, 28, percent(0)),
		},
	), (
		3,
		pallet_referenda::TrackInfo {
			name: "fellowship_admin",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(17, 28, percent(50)),
			min_support: Curve::make_reciprocal(12, 28, percent(1), percent(0)),
		},
	), (
		4,
		pallet_referenda::TrackInfo {
			name: "general_admin",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_reciprocal(4, 28, percent(80), percent(50)),
			min_support: Curve::make_reciprocal(7, 28, percent(10), percent(0)),
		},
	), (
		5,
		pallet_referenda::TrackInfo {
			name: "auction_admin",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_reciprocal(4, 28, percent(80), percent(50)),
			min_support: Curve::make_reciprocal(7, 28, percent(10), percent(0)),
		},
	), (
		6,
		pallet_referenda::TrackInfo {
			name: "lease_admin",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(17, 28, percent(50)),
			min_support: Curve::make_reciprocal(12, 28, percent(1), percent(0)),
		},
	), (
		7,
		pallet_referenda::TrackInfo {
			name: "referendum_canceller",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(17, 28, percent(50)),
			min_support: Curve::make_reciprocal(12, 28, percent(1), percent(0)),
		},
	), (
		8,
		pallet_referenda::TrackInfo {
			name: "referendum_killer",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(17, 28, percent(50)),
			min_support: Curve::make_reciprocal(12, 28, percent(1), percent(0)),
		},
	), (
		9,
		pallet_referenda::TrackInfo {
			name: "small_tipper",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(10, 28, percent(50)),
			min_support: Curve::make_reciprocal(1, 28, percent(4), percent(0)),
		},
	), (
		10,
		pallet_referenda::TrackInfo {
			name: "big_tipper",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(10, 28, percent(50)),
			min_support: Curve::make_reciprocal(8, 28, percent(1), percent(0)),
		},
	), (
		11,
		pallet_referenda::TrackInfo {
			name: "small_spender",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(17, 28, percent(50)),
			min_support: Curve::make_reciprocal(12, 28, percent(1), percent(0)),
		},
	), (
		12,
		pallet_referenda::TrackInfo {
			name: "medium_spender",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(23, 28, percent(50)),
			min_support: Curve::make_reciprocal(16, 28, percent(1), percent(0)),
		},
	), (
		13,
		pallet_referenda::TrackInfo {
			name: "big_spender",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_linear(28, 28, percent(50)),
			min_support: Curve::make_reciprocal(20, 28, percent(1), percent(0)),
		},
	), (
		14,
		pallet_referenda::TrackInfo {
			name: "whitelisted_caller",
			max_deciding: 1,
			decision_deposit: 10,
			prepare_period: 4,
			decision_period: 4,
			confirm_period: 2,
			min_enactment_period: 4,
			min_approval: Curve::make_reciprocal(16, 28 * 24, percent(96), percent(50)),
			min_support: Curve::make_reciprocal(1, 28, percent(20), percent(10)),
		},
	)
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
				StakingAdmin => Ok(1),
				Treasurer => Ok(2),
				FellowshipAdmin => Ok(3),
				GeneralAdmin => Ok(4),
				AuctionAdmin => Ok(5),
				LeaseAdmin => Ok(6),
				ReferendumCanceller => Ok(7),
				ReferendumKiller => Ok(8),
				SmallTipper => Ok(9),
				BigTipper => Ok(10),
				SmallSpender => Ok(11),
				MediumSpender => Ok(12),
				BigSpender => Ok(13),
				WhitelistedCaller => Ok(14),
				_ => Err(()),
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
