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

use primitives::ValidationCode;

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.
use super::{
	BackedCandidate, Config, DisputeStatementSet, UncheckedSignedAvailabilityBitfield, Weight,
};

pub trait WeightInfo {
	/// Variant over `v`, the count of dispute statements in a dispute statement set. This gives the
	/// weight of a single dispute statement set.
	fn enter_variable_disputes(v: u32) -> Weight;
	/// The weight of one bitfield.
	fn enter_bitfields() -> Weight;
	/// Variant over:
	/// - `v`, the count of validity votes for a backed candidate.
	/// - ump, total size of upward messages in bytes
	/// - htmp, total size of horizontal messages in bytes
	/// - code, size of new validation code if any in bytes
	/// of a single backed candidate.
	fn enter_backed_candidate(v: u32, ump: u32, hrmp: u32, code: u32) -> Weight;
}

pub struct TestWeightInfo;
// `WeightInfo` impl for unit and integration tests. Based off of the `max_block` weight for the
//  mock.
#[cfg(not(feature = "runtime-benchmarks"))]
impl WeightInfo for TestWeightInfo {
	fn enter_variable_disputes(v: u32) -> Weight {
		// MAX Block Weight should fit 4 disputes
		Weight::from_parts(80_000 * v as u64 + 80_000, 0)
	}
	fn enter_bitfields() -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		Weight::from_parts(40_000u64, 0)
	}
	fn enter_backed_candidate(v: u32, _ump: u32, _hrmp: u32, _code: u32) -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		Weight::from_parts(40_000 * v as u64 + 40_000, 0)
	}
}
// To simplify benchmarks running as tests, we set all the weights to 0. `enter` will exit early
// when if the data causes it to be over weight, but we don't want that to block a benchmark from
// running as a test.
#[cfg(feature = "runtime-benchmarks")]
impl WeightInfo for TestWeightInfo {
	fn enter_variable_disputes(_v: u32) -> Weight {
		Weight::zero()
	}
	fn enter_bitfields() -> Weight {
		Weight::zero()
	}
	fn enter_backed_candidate(_v: u32, _ump: u32, _hrmp: u32, _code: u32) -> Weight {
		Weight::zero()
	}
}

pub fn paras_inherent_total_weight<T: Config>(
	backed_candidates: &[BackedCandidate<<T as frame_system::Config>::Hash>],
	bitfields: &[UncheckedSignedAvailabilityBitfield],
	disputes: &[DisputeStatementSet],
) -> Weight {
	backed_candidates_weight::<T>(backed_candidates)
		.saturating_add(signed_bitfields_weight::<T>(bitfields.len()))
		.saturating_add(multi_dispute_statement_sets_weight::<T, _, _>(disputes))
}

pub fn dispute_statement_set_weight<T: Config, S: AsRef<DisputeStatementSet>>(
	statement_set: S,
) -> Weight {
	<<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(
		statement_set.as_ref().statements.len() as u32,
	)
}

pub fn multi_dispute_statement_sets_weight<
	T: Config,
	D: AsRef<[S]>,
	S: AsRef<DisputeStatementSet>,
>(
	disputes: D,
) -> Weight {
	disputes
		.as_ref()
		.iter()
		.map(|d| dispute_statement_set_weight::<T, &S>(d))
		.fold(Weight::zero(), |acc_weight, weight| acc_weight.saturating_add(weight))
}

pub fn signed_bitfields_weight<T: Config>(bitfields_len: usize) -> Weight {
	<<T as Config>::WeightInfo as WeightInfo>::enter_bitfields()
		.saturating_mul(bitfields_len as u64)
}

pub fn backed_candidate_weight<T: frame_system::Config + Config>(
	candidate: &BackedCandidate<T::Hash>,
) -> Weight {
	let v = candidate.validity_votes.len() as u32;
	let ump = candidate
		.candidate
		.commitments
		.upward_messages
		.iter()
		.map(|m| m.len() as u32)
		.sum();
	let hrmp = candidate
		.candidate
		.commitments
		.horizontal_messages
		.iter()
		.map(|m| m.data.len() as u32)
		.sum();
	let code = match &candidate.candidate.commitments.new_validation_code {
		Some(ValidationCode(upgrade)) => upgrade.len() as u32,
		None => 0,
	};
	<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidate(v, ump, hrmp, code)
}

pub fn backed_candidates_weight<T: frame_system::Config + Config>(
	candidates: &[BackedCandidate<T::Hash>],
) -> Weight {
	candidates
		.iter()
		.map(|c| backed_candidate_weight::<T>(c))
		.fold(Weight::zero(), |acc, x| acc.saturating_add(x))
}
