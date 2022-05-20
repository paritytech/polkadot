//! Chain specific types for polkadot, kusama and westend.

use crate::prelude::*;
use frame_support::{traits::ConstU32, weights::Weight};

pub mod westend {
	use super::*;

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	#[derive(Debug)]
	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MaxLength;
		type MaxWeight = MaxWeight;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type Solution = NposSolution16;

		fn solution_weight(v: u32, _t: u32, a: u32, d: u32) -> Weight {
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

	#[subxt::subxt(
		runtime_metadata_path = "artifacts/westend.scale",
		derive_for_all_types = "Clone, Debug, PartialEq",
		derive_for_type(type = "sp_core::crypto::AccountId32", derive = "Eq, Ord, PartialOrd")
	)]
	pub mod runtime {
		#[subxt(substitute_type = "westend_runtime::NposCompactSolution16")]
		use crate::chain::westend::NposSolution16;

		#[subxt(substitute_type = "sp_arithmetic::per_things::PerU16")]
		use sp_runtime::PerU16;

		#[subxt(substitute_type = "pallet_election_provider_multi_phase::RawSolution")]
		use pallet_election_provider_multi_phase::RawSolution;

		#[subxt(substitute_type = "sp_npos_elections::ElectionScore")]
		use sp_npos_elections::ElectionScore;

		#[subxt(substitute_type = "pallet_election_provider_multi_phase::Phase")]
		use pallet_election_provider_multi_phase::Phase;
	}

	pub use runtime::runtime_types;

	pub type ExtrinsicParams = subxt::PolkadotExtrinsicParamsBuilder<subxt::DefaultConfig>;

	pub type RuntimeApi = runtime::RuntimeApi<
		subxt::DefaultConfig,
		subxt::PolkadotExtrinsicParams<subxt::DefaultConfig>,
	>;

	pub mod epm {
		pub use super::{
			runtime::election_provider_multi_phase::*,
			runtime_types::pallet_election_provider_multi_phase::*,
		};
	}
}

pub mod polkadot {
	use crate::prelude::*;
	use frame_support::{traits::ConstU32, weights::Weight};

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution16::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(16)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MaxLength;
		type MaxWeight = MaxWeight;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type Solution = NposSolution16;

		fn solution_weight(v: u32, _t: u32, a: u32, d: u32) -> Weight {
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

	#[subxt::subxt(
		runtime_metadata_path = "artifacts/polkadot.scale",
		derive_for_all_types = "Clone, Debug, PartialEq",
		derive_for_type(type = "sp_core::crypto::AccountId32", derive = "Eq, Ord, PartialOrd")
	)]
	pub mod runtime {
		#[subxt(substitute_type = "polkadot_runtime::NposCompactSolution16")]
		use crate::chain::polkadot::NposSolution16;

		#[subxt(substitute_type = "sp_arithmetic::per_things::PerU16")]
		use sp_runtime::PerU16;

		#[subxt(substitute_type = "pallet_election_provider_multi_phase::RawSolution")]
		use pallet_election_provider_multi_phase::RawSolution;

		#[subxt(substitute_type = "sp_npos_elections::ElectionScore")]
		use sp_npos_elections::ElectionScore;

		#[subxt(substitute_type = "pallet_election_provider_multi_phase::Phase")]
		use pallet_election_provider_multi_phase::Phase;
	}

	pub use runtime::runtime_types;

	pub type ExtrinsicParams = subxt::PolkadotExtrinsicParamsBuilder<subxt::DefaultConfig>;

	pub type RuntimeApi = runtime::RuntimeApi<
		subxt::DefaultConfig,
		subxt::PolkadotExtrinsicParams<subxt::DefaultConfig>,
	>;

	pub mod epm {
		pub use super::{
			runtime::election_provider_multi_phase::*,
			runtime_types::pallet_election_provider_multi_phase::*,
		};
	}
}

pub mod kusama {
	use crate::prelude::*;
	use frame_support::{traits::ConstU32, weights::Weight};

	frame_election_provider_support::generate_solution_type!(
		#[compact]
		pub struct NposSolution24::<
			VoterIndex = u32,
			TargetIndex = u16,
			Accuracy = sp_runtime::PerU16,
			MaxVoters = ConstU32::<22500>
		>(24)
	);

	pub struct Config;

	impl pallet_election_provider_multi_phase::unsigned::MinerConfig for Config {
		type AccountId = AccountId;
		type MaxLength = MaxLength;
		type MaxWeight = MaxWeight;
		type MaxVotesPerVoter = MaxVotesPerVoter;
		type Solution = NposSolution24;

		fn solution_weight(v: u32, _t: u32, a: u32, d: u32) -> Weight {
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

	#[subxt::subxt(
		runtime_metadata_path = "artifacts/kusama.scale",
		derive_for_all_types = "Clone, Debug, PartialEq",
		derive_for_type(type = "sp_core::crypto::AccountId32", derive = "Eq, Ord, PartialOrd")
	)]
	pub mod runtime {
		#[subxt(substitute_type = "kusama_runtime::NposCompactSolution24")]
		use crate::chain::kusama::NposSolution24;

		#[subxt(substitute_type = "pallet_election_provider_multi_phase::RawSolution")]
		use pallet_election_provider_multi_phase::RawSolution;

		#[subxt(substitute_type = "sp_arithmetic::per_things::PerU16")]
		use sp_runtime::PerU16;

		#[subxt(substitute_type = "sp_npos_elections::ElectionScore")]
		use sp_npos_elections::ElectionScore;

		#[subxt(substitute_type = "pallet_election_provider_multi_phase::Phase")]
		use pallet_election_provider_multi_phase::Phase;
	}

	pub use runtime::runtime_types;

	pub type ExtrinsicParams = subxt::PolkadotExtrinsicParamsBuilder<subxt::DefaultConfig>;

	pub type RuntimeApi = runtime::RuntimeApi<
		subxt::DefaultConfig,
		subxt::PolkadotExtrinsicParams<subxt::DefaultConfig>,
	>;

	pub mod epm {
		pub use super::{
			runtime::election_provider_multi_phase::*,
			runtime_types::pallet_election_provider_multi_phase::*,
		};
	}
}
