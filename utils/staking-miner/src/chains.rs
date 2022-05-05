use crate::prelude::*;
use codec::{Decode, Encode};
use frame_support::{parameter_types, traits::ConstU32, weights::Weight};
use sp_runtime::{PerU16, Perbill};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

parameter_types! {
	pub BlockWeights: frame_system::limits::BlockWeights = frame_system::limits::BlockWeights
		::with_sensible_defaults(2 * frame_support::weights::constants::WEIGHT_PER_SECOND, NORMAL_DISPATCH_RATIO);
	pub static MinerMaxWeight: Weight = BlockWeights::get().max_block;
	// TODO: align with the chains
	pub static MinerMaxLength: u32 = 256;
	// TODO: align with the chains
	pub static MinerMaxVotesPerVoter: u32 = 256;
}

pub mod westend {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500>
		>(24)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MinerMaxLength;
		type MaxWeight = MinerMaxWeight;
		type MaxVotesPerVoter = MinerMaxVotesPerVoter;
		type Solution = NposSolution24;

		fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
			// feasibility weight.
			(31_722_000 as Weight)
				// Standard Error: 8_000
				.saturating_add((1_255_000 as Weight).saturating_mul(v as Weight))
				// Standard Error: 28_000
				.saturating_add((8_972_000 as Weight).saturating_mul(a as Weight))
				// Standard Error: 42_000
				.saturating_add((966_000 as Weight).saturating_mul(d as Weight))
			//.saturating_add(T::DbWeight::get().reads(4 as Weight))
		}
	}
}

pub mod polkadot {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MinerMaxLength;
		type MaxWeight = MinerMaxWeight;
		type MaxVotesPerVoter = MinerMaxVotesPerVoter;
		type Solution = polkadot::NposSolution16;

		fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
			// feasibility weight.
			(31_722_000 as Weight)
				// Standard Error: 8_000
				.saturating_add((1_255_000 as Weight).saturating_mul(v as Weight))
				// Standard Error: 28_000
				.saturating_add((8_972_000 as Weight).saturating_mul(a as Weight))
				// Standard Error: 42_000
				.saturating_add((966_000 as Weight).saturating_mul(d as Weight))
			//.saturating_add(T::DbWeight::get().reads(4 as Weight))
		}
	}
}

pub mod kusama {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = PerU16,
			MaxVoters = ConstU32::<22500>
		>(24)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MinerMaxLength;
		type MaxWeight = MinerMaxWeight;
		type MaxVotesPerVoter = MinerMaxVotesPerVoter;
		type Solution = NposSolution24;

		fn solution_weight(v: u32, t: u32, a: u32, d: u32) -> Weight {
			// feasibility weight.
			(31_722_000 as Weight)
				// Standard Error: 8_000
				.saturating_add((1_255_000 as Weight).saturating_mul(v as Weight))
				// Standard Error: 28_000
				.saturating_add((8_972_000 as Weight).saturating_mul(a as Weight))
				// Standard Error: 42_000
				.saturating_add((966_000 as Weight).saturating_mul(d as Weight))
			//.saturating_add(T::DbWeight::get().reads(4 as Weight))
		}
	}
}
