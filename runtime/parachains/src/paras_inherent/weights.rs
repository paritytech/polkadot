// Copyright (C) Parity Technologies (UK) Ltd.
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

//! We use benchmarks to get time weights, for proof_size we manually use the size of the input
//! data, which will be part of the block. This is because we don't care about the storage proof on
//! the relay chain, but we do care about the size of the block, by putting the tx in the
//! proof_size we can use the already existing weight limiting code to limit the used size as well.

use parity_scale_codec::{Encode, WrapperTypeEncode};
use primitives::{
	CheckedMultiDisputeStatementSet, MultiDisputeStatementSet, UncheckedSignedAvailabilityBitfield,
	UncheckedSignedAvailabilityBitfields,
};

use super::{BackedCandidate, Config, DisputeStatementSet, Weight};

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
		Weight::from_parts(80_000 * v as u64 + 80_000, 0)
	}
	fn enter_bitfields() -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		Weight::from_parts(40_000u64, 0)
	}
	fn enter_backed_candidates_variable(v: u32) -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		Weight::from_parts(40_000 * v as u64 + 40_000, 0)
	}
	fn enter_backed_candidate_code_upgrade() -> Weight {
		Weight::zero()
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
	fn enter_backed_candidates_variable(_v: u32) -> Weight {
		Weight::zero()
	}
	fn enter_backed_candidate_code_upgrade() -> Weight {
		Weight::zero()
	}
}

pub fn paras_inherent_total_weight<T: Config>(
	backed_candidates: &[BackedCandidate<<T as frame_system::Config>::Hash>],
	bitfields: &UncheckedSignedAvailabilityBitfields,
	disputes: &MultiDisputeStatementSet,
) -> Weight {
	backed_candidates_weight::<T>(backed_candidates)
		.saturating_add(signed_bitfields_weight::<T>(bitfields))
		.saturating_add(multi_dispute_statement_sets_weight::<T>(disputes))
}

pub fn multi_dispute_statement_sets_weight<T: Config>(
	disputes: &MultiDisputeStatementSet,
) -> Weight {
	set_proof_size_to_tx_size(
		disputes
			.iter()
			.map(|d| dispute_statement_set_weight::<T, _>(d))
			.fold(Weight::zero(), |acc_weight, weight| acc_weight.saturating_add(weight)),
		disputes,
	)
}

pub fn checked_multi_dispute_statement_sets_weight<T: Config>(
	disputes: &CheckedMultiDisputeStatementSet,
) -> Weight {
	set_proof_size_to_tx_size(
		disputes
			.iter()
			.map(|d| dispute_statement_set_weight::<T, _>(d))
			.fold(Weight::zero(), |acc_weight, weight| acc_weight.saturating_add(weight)),
		disputes,
	)
}

/// Get time weights from benchmarks and set proof size to tx size.
pub fn dispute_statement_set_weight<T, D>(statement_set: D) -> Weight
where
	T: Config,
	D: AsRef<DisputeStatementSet> + WrapperTypeEncode + Sized + Encode,
{
	set_proof_size_to_tx_size(
		<<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(
			statement_set.as_ref().statements.len() as u32,
		),
		statement_set,
	)
}

pub fn signed_bitfields_weight<T: Config>(
	bitfields: &UncheckedSignedAvailabilityBitfields,
) -> Weight {
	set_proof_size_to_tx_size(
		<<T as Config>::WeightInfo as WeightInfo>::enter_bitfields()
			.saturating_mul(bitfields.len() as u64),
		bitfields,
	)
}

pub fn signed_bitfield_weight<T: Config>(bitfield: &UncheckedSignedAvailabilityBitfield) -> Weight {
	set_proof_size_to_tx_size(
		<<T as Config>::WeightInfo as WeightInfo>::enter_bitfields(),
		bitfield,
	)
}

pub fn backed_candidate_weight<T: frame_system::Config + Config>(
	candidate: &BackedCandidate<T::Hash>,
) -> Weight {
	set_proof_size_to_tx_size(
		if candidate.candidate.commitments.new_validation_code.is_some() {
			<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidate_code_upgrade()
		} else {
			<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidates_variable(
				candidate.validity_votes.len() as u32,
			)
		},
		candidate,
	)
}

pub fn backed_candidates_weight<T: frame_system::Config + Config>(
	candidates: &[BackedCandidate<T::Hash>],
) -> Weight {
	candidates
		.iter()
		.map(|c| backed_candidate_weight::<T>(c))
		.fold(Weight::zero(), |acc, x| acc.saturating_add(x))
}

/// Set proof_size component of `Weight` to tx size.
fn set_proof_size_to_tx_size<Arg: Encode>(weight: Weight, arg: Arg) -> Weight {
	weight.set_proof_size(arg.encoded_size() as u64)
}
