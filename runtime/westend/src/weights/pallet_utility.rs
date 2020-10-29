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
//! Weights for pallet_utility
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0
//! DATE: 2020-09-28, STEPS: [50], REPEAT: 20, LOW RANGE: [], HIGH RANGE: []

#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Trait> pallet_utility::WeightInfo for WeightInfo<T> {
	fn batch(c: u32, ) -> Weight {
		(18_589_000 as Weight)
			.saturating_add((1_734_000 as Weight).saturating_mul(c as Weight))
	}
	fn as_derivative() -> Weight {
		(5_611_000 as Weight)
	}
	fn batch_all(c: u32, ) -> Weight {
		(21_104_000 as Weight)
			.saturating_add((1_509_000 as Weight).saturating_mul(c as Weight))
	}
}
