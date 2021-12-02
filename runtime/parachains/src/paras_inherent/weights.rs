// Copyright 2021 Parity Technologies (UK) Ltd.
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
use super::{
	cheap_bitfield_checks, BackedCandidate, Config, DisputeStatementSet,
	UncheckedSignedAvailabilityBitfield, Weight,
};
use crate::{
	configuration::HostConfiguration,
	inclusion::{self, FullCheck},
};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
const MAX_UNCHECKED_BITFIELD_ITERATIONS: usize = 1_000;

pub trait WeightInfo {
	/// Variant over `v`, the count of dispute statements in a dispute statement set. This gives the
	/// weight of a single dispute statement set.
	fn enter_variable_disputes(v: u32) -> Weight;
	/// The weight of one bitfield.
	fn enter_bitfields() -> Weight;
	/// Variant over `v`, the count of validity votes for a backed candidate. This gives the weight
	/// of a single backed candidate.
	fn enter_backed_candidates_variable(v: u32) -> Weight;
	/// The weight of a single backed candidate with a code upgrade.
	fn enter_backed_candidate_code_upgrade() -> Weight;
}

pub struct TestWeightInfo;
// `WeightInfo` impl for unit and integration tests. Based off of the `max_block` weight for the
//  mock.
#[cfg(not(feature = "runtime-benchmarks"))]
impl WeightInfo for TestWeightInfo {
	fn enter_variable_disputes(v: u32) -> Weight {
		// MAX Block Weight should fit 4 disputes
		80_000 * v as Weight + 80_000
	}
	fn enter_bitfields() -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		40_000 as Weight
	}
	fn enter_backed_candidates_variable(v: u32) -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		40_000 * v as Weight + 40_000
	}
	fn enter_backed_candidate_code_upgrade() -> Weight {
		0
	}
}
// To simplify benchmarks running as tests, we set all the weights to 0. `enter` will exit early
// when if the data causes it to be over weight, but we don't want that to block a benchmark from
// running as a test.
#[cfg(feature = "runtime-benchmarks")]
impl WeightInfo for TestWeightInfo {
	fn enter_variable_disputes(_v: u32) -> Weight {
		0
	}
	fn enter_bitfields() -> Weight {
		0
	}
	fn enter_backed_candidates_variable(_v: u32) -> Weight {
		0
	}
	fn enter_backed_candidate_code_upgrade() -> Weight {
		0
	}
}

// OR together all bitfields and count the ones from the result.
// Note that this will only OR together the first `MAX_UNCHECKED_BITFIELD_ITERATIONS` to avoid
// excessive iteration.
pub fn bitfields_count_ones(
	bitfields: &[UncheckedSignedAvailabilityBitfield],
	expected_bits: usize,
	validator_count: usize,
) -> usize {
	let mut last_index = None;
	bitfields
		.iter()
		.take(MAX_UNCHECKED_BITFIELD_ITERATIONS)
		.fold(
			BitVec::<BitOrderLsb0, u8>::repeat(false, expected_bits),
			|acc: BitVec<BitOrderLsb0, u8>, cur| {
				if cheap_bitfield_checks(
					cur,
					expected_bits,
					validator_count,
					last_index,
					&FullCheck::Skip,
				) {
					last_index = Some(cur.unchecked_validator_index());
					acc | cur.unchecked_payload().0.clone()
				} else {
					acc
				}
			},
		)
		.count_ones()
}

// Note that this does a storage read.
pub fn paras_inherent_total_weight<T: Config>(
	backed_candidates: &[BackedCandidate<<T as frame_system::Config>::Hash>],
	bitfields: &[UncheckedSignedAvailabilityBitfield],
	disputes: &[DisputeStatementSet],
	expected_bits: usize,
	validator_count: usize,
	config: &HostConfiguration<T::BlockNumber>,
) -> Weight {
	backed_candidates_weight::<T>(backed_candidates)
		.saturating_add(signed_bitfields_weight::<T>(bitfields.len()))
		.saturating_add(dispute_statements_weight::<T>(disputes))
		.saturating_add(enact_candidates_weight::<T>(
			bitfields_count_ones(bitfields, expected_bits, validator_count) as u32,
			config,
		))
}

pub fn dispute_statements_weight<T: Config>(disputes: &[DisputeStatementSet]) -> Weight {
	disputes
		.iter()
		.map(|d| {
			<<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(
				d.statements.len() as u32
			)
		})
		.fold(0, |acc, x| acc.saturating_add(x))
}

pub fn signed_bitfields_weight<T: Config>(bitfields_len: usize) -> Weight {
	<<T as Config>::WeightInfo as WeightInfo>::enter_bitfields()
		.saturating_mul(bitfields_len as Weight)
}

// Calculate the worst case weight for enacting the given number of candidates.
pub fn enact_candidates_weight<T: frame_system::Config + Config>(
	candidate_count: u32,
	config: &HostConfiguration<T::BlockNumber>,
) -> Weight {
	<inclusion::Pallet<T>>::enact_candidate_weight(
		config.hrmp_max_parathread_inbound_channels,
		config.hrmp_max_parachain_inbound_channels,
		config.max_upward_message_num_per_candidate,
		config.hrmp_max_message_num_per_candidate,
	)
	.saturating_mul(candidate_count as Weight)
}

pub fn backed_candidate_weight<T: frame_system::Config + Config>(
	candidate: &BackedCandidate<T::Hash>,
) -> Weight {
	if candidate.candidate.commitments.new_validation_code.is_some() {
		<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidate_code_upgrade()
	} else {
		<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidates_variable(
			candidate.validity_votes.len() as u32,
		)
	}
}

// Calculate the max weight of the given candidates.
pub fn backed_candidates_weight<T: frame_system::Config + Config>(
	candidates: &[BackedCandidate<T::Hash>],
) -> Weight {
	candidates
		.iter()
		.map(|c| backed_candidate_weight::<T>(c))
		.fold(0, |acc, x| acc.saturating_add(x))
}
