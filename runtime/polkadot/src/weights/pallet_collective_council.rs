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
//! Autogenerated weights for `pallet_collective`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-02-22, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `bm4`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("polkadot-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=polkadot-dev
// --steps=50
// --repeat=20
// --pallet=pallet_collective
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/polkadot/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_collective`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_collective::WeightInfo for WeightInfo<T> {
	/// Storage: Council Members (r:1 w:1)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Proposals (r:1 w:0)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Voting (r:100 w:100)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Prime (r:0 w:1)
	/// Proof Skipped: Council Prime (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `m` is `[0, 100]`.
	/// The range of component `n` is `[0, 100]`.
	/// The range of component `p` is `[0, 100]`.
	/// The range of component `m` is `[0, 100]`.
	/// The range of component `n` is `[0, 100]`.
	/// The range of component `p` is `[0, 100]`.
	fn set_members(m: u32, _n: u32, p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0 + m * (3233 ±0) + p * (3223 ±0)`
		//  Estimated: `16054 + m * (7809 ±16) + p * (10238 ±16)`
		// Minimum execution time: 16_977 nanoseconds.
		Weight::from_parts(17_264_000, 0)
			.saturating_add(Weight::from_parts(0, 16054))
			// Standard Error: 47_984
			.saturating_add(Weight::from_parts(5_491_508, 0).saturating_mul(m.into()))
			// Standard Error: 47_984
			.saturating_add(Weight::from_parts(7_895_249, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(p.into())))
			.saturating_add(T::DbWeight::get().writes(2))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(p.into())))
			.saturating_add(Weight::from_parts(0, 7809).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 10238).saturating_mul(p.into()))
	}
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[1, 100]`.
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[1, 100]`.
	fn execute(b: u32, m: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `101 + m * (32 ±0)`
		//  Estimated: `597 + m * (32 ±0)`
		// Minimum execution time: 15_072 nanoseconds.
		Weight::from_parts(14_211_059, 0)
			.saturating_add(Weight::from_parts(0, 597))
			// Standard Error: 23
			.saturating_add(Weight::from_parts(1_603, 0).saturating_mul(b.into()))
			// Standard Error: 242
			.saturating_add(Weight::from_parts(17_712, 0).saturating_mul(m.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(Weight::from_parts(0, 32).saturating_mul(m.into()))
	}
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:1 w:0)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[1, 100]`.
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[1, 100]`.
	fn propose_execute(b: u32, m: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `101 + m * (32 ±0)`
		//  Estimated: `3174 + m * (64 ±0)`
		// Minimum execution time: 17_130 nanoseconds.
		Weight::from_parts(16_211_186, 0)
			.saturating_add(Weight::from_parts(0, 3174))
			// Standard Error: 30
			.saturating_add(Weight::from_parts(1_785, 0).saturating_mul(b.into()))
			// Standard Error: 318
			.saturating_add(Weight::from_parts(25_434, 0).saturating_mul(m.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(Weight::from_parts(0, 64).saturating_mul(m.into()))
	}
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:1 w:1)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Proposals (r:1 w:1)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalCount (r:1 w:1)
	/// Proof Skipped: Council ProposalCount (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Voting (r:0 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[2, 100]`.
	/// The range of component `p` is `[1, 100]`.
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[2, 100]`.
	/// The range of component `p` is `[1, 100]`.
	fn propose_proposed(b: u32, m: u32, p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `423 + m * (32 ±0) + p * (36 ±0)`
		//  Estimated: `5690 + m * (165 ±0) + p * (180 ±0)`
		// Minimum execution time: 22_253 nanoseconds.
		Weight::from_parts(23_501_095, 0)
			.saturating_add(Weight::from_parts(0, 5690))
			// Standard Error: 51
			.saturating_add(Weight::from_parts(2_472, 0).saturating_mul(b.into()))
			// Standard Error: 541
			.saturating_add(Weight::from_parts(18_687, 0).saturating_mul(m.into()))
			// Standard Error: 534
			.saturating_add(Weight::from_parts(93_359, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(4))
			.saturating_add(Weight::from_parts(0, 165).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 180).saturating_mul(p.into()))
	}
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Voting (r:1 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// The range of component `m` is `[5, 100]`.
	/// The range of component `m` is `[5, 100]`.
	fn vote(m: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `873 + m * (64 ±0)`
		//  Estimated: `4714 + m * (128 ±0)`
		// Minimum execution time: 20_932 nanoseconds.
		Weight::from_parts(21_526_308, 0)
			.saturating_add(Weight::from_parts(0, 4714))
			// Standard Error: 221
			.saturating_add(Weight::from_parts(40_562, 0).saturating_mul(m.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(Weight::from_parts(0, 128).saturating_mul(m.into()))
	}
	/// Storage: Council Voting (r:1 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Proposals (r:1 w:1)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:0 w:1)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	fn close_early_disapproved(m: u32, p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `493 + m * (64 ±0) + p * (36 ±0)`
		//  Estimated: `5361 + m * (260 ±0) + p * (144 ±0)`
		// Minimum execution time: 25_034 nanoseconds.
		Weight::from_parts(26_214_468, 0)
			.saturating_add(Weight::from_parts(0, 5361))
			// Standard Error: 408
			.saturating_add(Weight::from_parts(20_710, 0).saturating_mul(m.into()))
			// Standard Error: 398
			.saturating_add(Weight::from_parts(86_893, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(Weight::from_parts(0, 260).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 144).saturating_mul(p.into()))
	}
	/// Storage: Council Voting (r:1 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:1 w:1)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Proposals (r:1 w:1)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	fn close_early_approved(b: u32, m: u32, p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `829 + b * (1 ±0) + m * (64 ±0) + p * (40 ±0)`
		//  Estimated: `8632 + b * (4 ±0) + m * (264 ±0) + p * (160 ±0)`
		// Minimum execution time: 34_338 nanoseconds.
		Weight::from_parts(37_215_148, 0)
			.saturating_add(Weight::from_parts(0, 8632))
			// Standard Error: 124
			.saturating_add(Weight::from_parts(2_091, 0).saturating_mul(b.into()))
			// Standard Error: 1_277
			.saturating_add(Weight::from_parts(121_586, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(Weight::from_parts(0, 4).saturating_mul(b.into()))
			.saturating_add(Weight::from_parts(0, 264).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 160).saturating_mul(p.into()))
	}
	/// Storage: Council Voting (r:1 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Prime (r:1 w:0)
	/// Proof Skipped: Council Prime (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Proposals (r:1 w:1)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:0 w:1)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	fn close_disapproved(m: u32, p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `513 + m * (64 ±0) + p * (36 ±0)`
		//  Estimated: `6430 + m * (325 ±0) + p * (180 ±0)`
		// Minimum execution time: 27_556 nanoseconds.
		Weight::from_parts(28_644_244, 0)
			.saturating_add(Weight::from_parts(0, 6430))
			// Standard Error: 439
			.saturating_add(Weight::from_parts(25_115, 0).saturating_mul(m.into()))
			// Standard Error: 428
			.saturating_add(Weight::from_parts(86_845, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(Weight::from_parts(0, 325).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 180).saturating_mul(p.into()))
	}
	/// Storage: Council Voting (r:1 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Members (r:1 w:0)
	/// Proof Skipped: Council Members (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Prime (r:1 w:0)
	/// Proof Skipped: Council Prime (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:1 w:1)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council Proposals (r:1 w:1)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	/// The range of component `b` is `[2, 1024]`.
	/// The range of component `m` is `[4, 100]`.
	/// The range of component `p` is `[1, 100]`.
	fn close_approved(b: u32, m: u32, p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `849 + b * (1 ±0) + m * (64 ±0) + p * (40 ±0)`
		//  Estimated: `9900 + b * (5 ±0) + m * (330 ±0) + p * (200 ±0)`
		// Minimum execution time: 37_125 nanoseconds.
		Weight::from_parts(38_591_779, 0)
			.saturating_add(Weight::from_parts(0, 9900))
			// Standard Error: 83
			.saturating_add(Weight::from_parts(2_325, 0).saturating_mul(b.into()))
			// Standard Error: 886
			.saturating_add(Weight::from_parts(22_146, 0).saturating_mul(m.into()))
			// Standard Error: 864
			.saturating_add(Weight::from_parts(118_663, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(Weight::from_parts(0, 5).saturating_mul(b.into()))
			.saturating_add(Weight::from_parts(0, 330).saturating_mul(m.into()))
			.saturating_add(Weight::from_parts(0, 200).saturating_mul(p.into()))
	}
	/// Storage: Council Proposals (r:1 w:1)
	/// Proof Skipped: Council Proposals (max_values: Some(1), max_size: None, mode: Measured)
	/// Storage: Council Voting (r:0 w:1)
	/// Proof Skipped: Council Voting (max_values: None, max_size: None, mode: Measured)
	/// Storage: Council ProposalOf (r:0 w:1)
	/// Proof Skipped: Council ProposalOf (max_values: None, max_size: None, mode: Measured)
	/// The range of component `p` is `[1, 100]`.
	/// The range of component `p` is `[1, 100]`.
	fn disapprove_proposal(p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `258 + p * (32 ±0)`
		//  Estimated: `1269 + p * (96 ±0)`
		// Minimum execution time: 14_192 nanoseconds.
		Weight::from_parts(16_135_343, 0)
			.saturating_add(Weight::from_parts(0, 1269))
			// Standard Error: 439
			.saturating_add(Weight::from_parts(82_641, 0).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(Weight::from_parts(0, 96).saturating_mul(p.into()))
	}
}
