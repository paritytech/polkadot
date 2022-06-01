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
use frame_support::{parameter_types, traits::EitherOf};
use frame_system::EnsureRootWithSuccess;

// Old governance configurations.
pub mod old;

mod origins;
pub use origins::{
	pallet_custom_origins,
	StakingAdmin, FellowshipAdmin, GeneralAdmin, AuctionAdmin, LeaseAdmin,
	ReferendumCanceller, ReferendumKiller, Spender,
	WhitelistedCaller, FellowshipInitiates, Fellows,
	FellowshipExperts, FellowshipMasters,
};
mod tracks;
pub use tracks::TracksInfo;
mod fellowship;
pub use fellowship::{FellowshipReferendaInstance, FellowshipCollectiveInstance};

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

parameter_types! {
	pub const MaxBalance: Balance = Balance::max_value();
}
pub type TreasurySpender = EitherOf< EnsureRootWithSuccess<AccountId, MaxBalance>, Spender >;

impl origins::pallet_custom_origins::Config for Runtime {}

impl pallet_whitelist::Config for Runtime {
	type WeightInfo = pallet_whitelist::weights::SubstrateWeight<Self>; //TODO
	type Event = Event;
	type Call = Call;
	type WhitelistOrigin = Fellows;
	type DispatchWhitelistedOrigin = WhitelistedCaller;
	type PreimageProvider = Preimage;
}

impl pallet_referenda::Config for Runtime {
	type WeightInfo = pallet_referenda::weights::SubstrateWeight<Self>; //TODO
	type Call = Call;
	type Event = Event;
	type Scheduler = Scheduler;
	type Currency = Balances;
	type SubmitOrigin = frame_system::EnsureSigned<AccountId>;
	type CancelOrigin = ReferendumCanceller;
	type KillOrigin = ReferendumKiller;
	type Slash = ();
	type Votes = pallet_conviction_voting::VotesOf<Runtime>;
	type Tally = pallet_conviction_voting::TallyOf<Runtime>;
	type SubmissionDeposit = SubmissionDeposit;
	type MaxQueued = ConstU32<100>;
	type UndecidingTimeout = UndecidingTimeout;
	type AlarmInterval = AlarmInterval;
	type Tracks = TracksInfo;
}
