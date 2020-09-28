// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Default weights for the Babe Pallet
//! This file was not auto-generated.

use frame_support::{
	traits::Get,
	weights::{
		Weight,
		constants::{WEIGHT_PER_MICROS, WEIGHT_PER_NANOS},
	}
};
use sp_std::marker::PhantomData;

pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Trait> pallet_grandpa::WeightInfo for WeightInfo<T> {
	fn report_equivocation(validator_count: u32) -> Weight {
		// we take the validator set count from the membership proof to
		// calculate the weight but we set a floor of 100 validators.
		let validator_count = validator_count.max(100) as u64;

		// worst case we are considering is that the given offender
		// is backed by 200 nominators
		const MAX_NOMINATORS: u64 = 200;

		// checking membership proof
		(35 * WEIGHT_PER_MICROS)
			.saturating_add((175 * WEIGHT_PER_NANOS).saturating_mul(validator_count))
			.saturating_add(T::DbWeight::get().reads(5))
			// check equivocation proof
			.saturating_add(95 * WEIGHT_PER_MICROS)
			// report offence
			.saturating_add(110 * WEIGHT_PER_MICROS)
			.saturating_add(25 * WEIGHT_PER_MICROS * MAX_NOMINATORS)
			.saturating_add(T::DbWeight::get().reads(14 + 3 * MAX_NOMINATORS))
			.saturating_add(T::DbWeight::get().writes(10 + 3 * MAX_NOMINATORS))
			// fetching set id -> session index mappings
			.saturating_add(T::DbWeight::get().reads(2))
	}

	fn note_stalled() -> Weight {
		(3 * WEIGHT_PER_MICROS)
			.saturating_add(T::DbWeight::get().writes(1))
	}
}
