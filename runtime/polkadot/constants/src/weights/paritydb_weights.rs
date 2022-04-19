// This file is part of Substrate.

// Copyright (C) 2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-03-30 (Y/M/D)
//!
//! DATABASE: `ParityDb`, RUNTIME: `Polkadot`
//! BLOCK-NUM: `BlockId::Number(9653477)`
//! SKIP-WRITE: `false`, SKIP-READ: `false`, WARMUPS: `1`
//! STATE-VERSION: `V0`, STATE-CACHE-SIZE: `0`
//! WEIGHT-PATH: `runtime/polkadot/constants/src/weights/`
//! METRIC: `Average`, WEIGHT-MUL: `1.1`, WEIGHT-ADD: `0`

// Executed Command:
//   ./target/production/polkadot
//   benchmark-storage
//   --db=paritydb
//   --state-version=0
//   --mul=1.1
//   --weight-path=runtime/polkadot/constants/src/weights/

/// Storage DB weights for the `Polkadot` runtime and `ParityDb`.
pub mod constants {
	use frame_support::{
		parameter_types,
		weights::{constants, RuntimeDbWeight},
	};

	parameter_types! {
		/// `ParityDB` can be enabled with a feature flag, but is still experimental. These weights
		/// are available for brave runtime engineers who may want to try this out as default.
		pub const ParityDbWeight: RuntimeDbWeight = RuntimeDbWeight {
			/// Time to read one storage item.
			/// Calculated by multiplying the *Average* of all values with `1.1` and adding `0`.
			///
			/// Stats [NS]:
			///   Min, Max: 4_611, 13_478_005
			///   Average:  10_750
			///   Median:   10_655
			///   Std-Dev:  12214.49
			///
			/// Percentiles [NS]:
			///   99th: 14_451
			///   95th: 12_588
			///   75th: 11_200
			read: 11_826 * constants::WEIGHT_PER_NANOS,

			/// Time to write one storage item.
			/// Calculated by multiplying the *Average* of all values with `1.1` and adding `0`.
			///
			/// Stats [NS]:
			///   Min, Max: 8_023, 47_367_740
			///   Average:  34_592
			///   Median:   32_703
			///   Std-Dev:  49417.24
			///
			/// Percentiles [NS]:
			///   99th: 69_379
			///   95th: 47_168
			///   75th: 35_252
			write: 38_052 * constants::WEIGHT_PER_NANOS,
		};
	}

	#[cfg(test)]
	mod test_db_weights {
		use super::constants::ParityDbWeight as W;
		use frame_support::weights::constants;

		/// Checks that all weights exist and have sane values.
		// NOTE: If this test fails but you are sure that the generated values are fine,
		// you can delete it.
		#[test]
		fn bound() {
			// At least 1 µs.
			assert!(
				W::get().reads(1) >= constants::WEIGHT_PER_MICROS,
				"Read weight should be at least 1 µs."
			);
			assert!(
				W::get().writes(1) >= constants::WEIGHT_PER_MICROS,
				"Write weight should be at least 1 µs."
			);
			// At most 1 ms.
			assert!(
				W::get().reads(1) <= constants::WEIGHT_PER_MILLIS,
				"Read weight should be at most 1 ms."
			);
			assert!(
				W::get().writes(1) <= constants::WEIGHT_PER_MILLIS,
				"Write weight should be at most 1 ms."
			);
		}
	}
}
