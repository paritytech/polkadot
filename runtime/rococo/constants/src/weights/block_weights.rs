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
//! HOSTNAME: `bm6`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//!
//! SHORT-NAME: `block`, LONG-NAME: `BlockExecution`, RUNTIME: `Development`
//! WARMUPS: `10`, REPEAT: `100`
//! WEIGHT-PATH: `runtime/rococo/constants/src/weights/`
//! WEIGHT-METRIC: `Average`, WEIGHT-MUL: `1.0`, WEIGHT-ADD: `0`

// Executed Command:
//   ./target/production/polkadot
//   benchmark
//   overhead
//   --chain=rococo-dev
//   --execution=wasm
//   --wasm-execution=compiled
//   --weight-path=runtime/rococo/constants/src/weights/
//   --warmup=10
//   --repeat=100
//   --header=./file_header.txt

use sp_core::parameter_types;
use sp_weights::{constants::WEIGHT_REF_TIME_PER_NANOS, Weight};

parameter_types! {
	/// Time to execute an empty block.
	/// Calculated by multiplying the *Average* with `1.0` and adding `0`.
	///
	/// Stats nanoseconds:
	///   Min, Max: 5_285_413, 5_582_840
	///   Average:  5_334_883
	///   Median:   5_320_357
	///   Std-Dev:  54133.56
	///
	/// Percentiles nanoseconds:
	///   99th: 5_495_378
	///   95th: 5_453_765
	///   75th: 5_352_587
	pub const BlockExecutionWeight: Weight =
		Weight::from_ref_time(WEIGHT_REF_TIME_PER_NANOS.saturating_mul(5_334_883));
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
			w.ref_time() >= 100u64 * constants::WEIGHT_REF_TIME_PER_MICROS,
			"Weight should be at least 100 µs."
		);
		// At most 50 ms.
		assert!(
			w.ref_time() <= 50u64 * constants::WEIGHT_REF_TIME_PER_MILLIS,
			"Weight should be at most 50 ms."
		);
	}
}
