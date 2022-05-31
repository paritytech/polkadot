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

//! Elements of governance concerning the Polkadot Fellowship. This is only a temporary arrangement
//! since the Polkadot Fellowship belongs under the Polkadot Relay. However, that is not yet in
//! place, so until then it will need to live here. Once it is in place and there exists a bridge
//! between Polkadot/Kusama then this code can be removed.

use super::*;
use crate::{QUID, DAYS};

parameter_types! {
	pub const AlarmInterval: BlockNumber = 1;
	pub const SubmissionDeposit: Balance = 100 * QUID;
	pub const UndecidingTimeout: BlockNumber = 7 * DAYS;
}

pub struct TracksInfo;
impl pallet_referenda::TracksInfo<Balance, BlockNumber> for TracksInfo {
	type Id = u16;
	type Origin = <Origin as frame_support::traits::OriginTrait>::PalletsOrigin;
	fn tracks() -> &'static [(Self::Id, pallet_referenda::TrackInfo<Balance, BlockNumber>)] {
		static DATA: [(u16, pallet_referenda::TrackInfo<Balance, BlockNumber>); 2] = [
			(0u16, pallet_referenda::TrackInfo {
				name: "public",
				max_deciding: 10,
				decision_deposit: 100 * QUID,
				prepare_period: 30 * MINUTES,
				decision_period: 7 * DAYS,
				confirm_period: 30 * MINUTES,
				min_enactment_period: 4,
				min_approval: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(50),
					ceil: Perbill::from_percent(100),
				},
				min_support: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(0),
					ceil: Perbill::from_percent(50),
				},
			}),
			(1u16, pallet_referenda::TrackInfo {
				name: "apprentices",
				max_deciding: 10,
				decision_deposit: 100 * QUID,
				prepare_period: 30 * MINUTES,
				decision_period: 7 * DAYS,
				confirm_period: 30 * MINUTES,
				min_enactment_period: 4,
				min_approval: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(50),
					ceil: Perbill::from_percent(100),
				},
				min_support: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(0),
					ceil: Perbill::from_percent(50),
				},
			}),
			(3u16, pallet_referenda::TrackInfo {
				name: "fellows",
				max_deciding: 10,
				decision_deposit: 100 * QUID,
				prepare_period: 30 * MINUTES,
				decision_period: 7 * DAYS,
				confirm_period: 30 * MINUTES,
				min_enactment_period: 4,
				min_approval: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(50),
					ceil: Perbill::from_percent(100),
				},
				min_support: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(0),
					ceil: Perbill::from_percent(50),
				},
			}),
			(5u16, pallet_referenda::TrackInfo {
				name: "masters",
				max_deciding: 10,
				decision_deposit: 100 * QUID,
				prepare_period: 30 * MINUTES,
				decision_period: 7 * DAYS,
				confirm_period: 30 * MINUTES,
				min_enactment_period: 4,
				min_approval: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(50),
					ceil: Perbill::from_percent(100),
				},
				min_support: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(0),
					ceil: Perbill::from_percent(50),
				},
			}),
			(7u16, pallet_referenda::TrackInfo {
				name: "elites",
				max_deciding: 10,
				decision_deposit: 100 * QUID,
				prepare_period: 30 * MINUTES,
				decision_period: 7 * DAYS,
				confirm_period: 30 * MINUTES,
				min_enactment_period: 4,
				min_approval: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(50),
					ceil: Perbill::from_percent(100),
				},
				min_support: pallet_referenda::Curve::LinearDecreasing {
					length: Perbill::from_percent(100),
					floor: Perbill::from_percent(0),
					ceil: Perbill::from_percent(50),
				},
			}),
		];
		&DATA[..]
	}
	fn track_for(id: &Self::Origin) -> Result<Self::Id, ()> {
		if let Ok(system_origin) = frame_system::RawOrigin::try_from(id.clone()) {
			match system_origin {
				FellowshipInitiates => Ok(0),
				FellowshipApprentices => Ok(1),
				Fellows => Ok(3),
				FellowshipMasters => Ok(5),
				FellowshipElites => Ok(7),
				_ => Err(()),
			}
		} else {
			Err(())
		}
	}
}

pub type FellowshipReferendaInstance = pallet_referenda::Instance2;

impl pallet_referenda::Config<FellowshipReferendaInstance> for Runtime {
	type WeightInfo = pallet_referenda::weights::SubstrateWeight<Self>;
	type Call = Call;
	type Event = Event;
	type Scheduler = Scheduler;
	type Currency = Balances;
	type CancelOrigin = FellowshipMasters<AccountId>;
	type KillOrigin = FellowshipElites<AccountId>;
	type Slash = ();
	type Votes = pallet_ranked_collective::Votes;
	type Tally = pallet_ranked_collective::TallyOf<Runtime>;
	type SubmissionDeposit = SubmissionDeposit;
	type MaxQueued = ConstU32<100>;
	type UndecidingTimeout = UndecidingTimeout;
	type AlarmInterval = AlarmInterval;
	type Tracks = TracksInfo;
}

pub type FellowshipCollectiveInstance = pallet_ranked_collective::Instance1;

impl pallet_ranked_collective::Config for Runtime {
	type WeightInfo = pallet_ranked_collective::weights::SubstrateWeight<Self>;
	type Event = Event;
	type AdminOrigin = FellowshipAdmin;
	type Polls = FellowshipReferenda;
	type VoteWeight = pallet_ranked_collective::Geometric;
}
