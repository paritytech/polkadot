// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-11-16 (Y/M/D)
//! HOSTNAME: `bm5`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//!
//! SHORT-NAME: `block`, LONG-NAME: `BlockExecution`, RUNTIME: `Development`
//! WARMUPS: `10`, REPEAT: `100`
//! WEIGHT-PATH: `runtime/kusama/constants/src/weights/`
//! WEIGHT-METRIC: `Average`, WEIGHT-MUL: `1.0`, WEIGHT-ADD: `0`

// Executed Command:
//   ./target/production/polkadot
//   benchmark
//   overhead
//   --chain=kusama-dev
//   --execution=wasm
//   --wasm-execution=compiled
//   --weight-path=runtime/kusama/constants/src/weights/
//   --warmup=10
//   --repeat=100
//   --header=./file_header.txt

use sp_core::parameter_types;
use sp_weights::{constants::WEIGHT_PER_NANOS, Weight};

parameter_types! {
	/// Time to execute an empty block.
	/// Calculated by multiplying the *Average* with `1.0` and adding `0`.
	///
	/// Stats nanoseconds:
	///   Min, Max: 6_665_440, 6_986_371
	///   Average:  6_731_894
	///   Median:   6_723_700
	///   Std-Dev:  49280.83
	///
	/// Percentiles nanoseconds:
	///   99th: 6_876_251
	///   95th: 6_811_463
	///   75th: 6_751_221
	pub const BlockExecutionWeight: Weight = WEIGHT_PER_NANOS.saturating_mul(6_731_894);
}

#[cfg(test)]
mod test_weights {
	use sp_weights::constants;

	/// Checks that the weight exists and is sane.
	// NOTE: If this test fails but you are sure that the generated values are fine,
	// you can delete it.
	#[test]
	fn sane() {
		let w = super::BlockExecutionWeight::get();

		// At least 100 µs.
		assert!(
			w.ref_time() >= 100u64 * constants::WEIGHT_PER_MICROS.ref_time(),
			"Weight should be at least 100 µs."
		);
		// At most 50 ms.
		assert!(
			w.ref_time() <= 50u64 * constants::WEIGHT_PER_MILLIS.ref_time(),
			"Weight should be at most 50 ms."
		);
	}
}
