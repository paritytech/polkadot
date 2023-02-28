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
//! Autogenerated weights for `pallet_identity`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2023-02-27, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `bm6`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=westend-dev
// --steps=50
// --repeat=20
// --pallet=pallet_identity
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_identity`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_identity::WeightInfo for WeightInfo<T> {
	/// Storage: Identity Registrars (r:1 w:1)
	/// Proof: Identity Registrars (max_values: Some(1), max_size: Some(1141), added: 1636, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 19]`.
	fn add_registrar(r: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `64 + r * (57 ±0)`
		//  Estimated: `1636`
		// Minimum execution time: 11_675 nanoseconds.
		Weight::from_ref_time(12_539_616)
			.saturating_add(Weight::from_proof_size(1636))
			// Standard Error: 1_875
			.saturating_add(Weight::from_ref_time(113_199).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity IdentityOf (r:1 w:1)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 20]`.
	/// The range of component `x` is `[0, 100]`.
	fn set_identity(r: u32, x: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `474 + r * (5 ±0)`
		//  Estimated: `10013`
		// Minimum execution time: 29_044 nanoseconds.
		Weight::from_ref_time(29_062_250)
			.saturating_add(Weight::from_proof_size(10013))
			// Standard Error: 5_693
			.saturating_add(Weight::from_ref_time(78_081).saturating_mul(r.into()))
			// Standard Error: 1_110
			.saturating_add(Weight::from_ref_time(458_837).saturating_mul(x.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity IdentityOf (r:1 w:0)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:100 w:100)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 100]`.
	fn set_subs_new(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `101`
		//  Estimated: `15746 + s * (2589 ±0)`
		// Minimum execution time: 9_098 nanoseconds.
		Weight::from_ref_time(22_624_884)
			.saturating_add(Weight::from_proof_size(15746))
			// Standard Error: 3_865
			.saturating_add(Weight::from_ref_time(2_735_607).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().reads((1_u64).saturating_mul(s.into())))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(s.into())))
			.saturating_add(Weight::from_proof_size(2589).saturating_mul(s.into()))
	}
	/// Storage: Identity IdentityOf (r:1 w:0)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:0 w:100)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// The range of component `p` is `[0, 100]`.
	fn set_subs_old(p: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `226 + p * (32 ±0)`
		//  Estimated: `15746`
		// Minimum execution time: 8_814 nanoseconds.
		Weight::from_ref_time(22_140_295)
			.saturating_add(Weight::from_proof_size(15746))
			// Standard Error: 3_600
			.saturating_add(Weight::from_ref_time(1_108_458).saturating_mul(p.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(p.into())))
	}
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// Storage: Identity IdentityOf (r:1 w:1)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:0 w:100)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 20]`.
	/// The range of component `s` is `[0, 100]`.
	/// The range of component `x` is `[0, 100]`.
	fn clear_identity(r: u32, s: u32, x: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `533 + r * (5 ±0) + s * (32 ±0) + x * (66 ±0)`
		//  Estimated: `15746`
		// Minimum execution time: 49_319 nanoseconds.
		Weight::from_ref_time(28_073_986)
			.saturating_add(Weight::from_proof_size(15746))
			// Standard Error: 6_192
			.saturating_add(Weight::from_ref_time(62_200).saturating_mul(r.into()))
			// Standard Error: 1_209
			.saturating_add(Weight::from_ref_time(1_122_770).saturating_mul(s.into()))
			// Standard Error: 1_209
			.saturating_add(Weight::from_ref_time(225_189).saturating_mul(x.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(s.into())))
	}
	/// Storage: Identity Registrars (r:1 w:0)
	/// Proof: Identity Registrars (max_values: Some(1), max_size: Some(1141), added: 1636, mode: MaxEncodedLen)
	/// Storage: Identity IdentityOf (r:1 w:1)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 20]`.
	/// The range of component `x` is `[0, 100]`.
	fn request_judgement(r: u32, x: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `431 + r * (57 ±0) + x * (66 ±0)`
		//  Estimated: `11649`
		// Minimum execution time: 30_208 nanoseconds.
		Weight::from_ref_time(28_731_902)
			.saturating_add(Weight::from_proof_size(11649))
			// Standard Error: 5_027
			.saturating_add(Weight::from_ref_time(127_894).saturating_mul(r.into()))
			// Standard Error: 981
			.saturating_add(Weight::from_ref_time(474_811).saturating_mul(x.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity IdentityOf (r:1 w:1)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 20]`.
	/// The range of component `x` is `[0, 100]`.
	fn cancel_request(r: u32, x: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `430 + x * (66 ±0)`
		//  Estimated: `10013`
		// Minimum execution time: 27_041 nanoseconds.
		Weight::from_ref_time(26_760_677)
			.saturating_add(Weight::from_proof_size(10013))
			// Standard Error: 7_122
			.saturating_add(Weight::from_ref_time(62_459).saturating_mul(r.into()))
			// Standard Error: 1_389
			.saturating_add(Weight::from_ref_time(468_208).saturating_mul(x.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity Registrars (r:1 w:1)
	/// Proof: Identity Registrars (max_values: Some(1), max_size: Some(1141), added: 1636, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 19]`.
	fn set_fee(r: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `121 + r * (57 ±0)`
		//  Estimated: `1636`
		// Minimum execution time: 7_140 nanoseconds.
		Weight::from_ref_time(7_733_950)
			.saturating_add(Weight::from_proof_size(1636))
			// Standard Error: 1_671
			.saturating_add(Weight::from_ref_time(110_862).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity Registrars (r:1 w:1)
	/// Proof: Identity Registrars (max_values: Some(1), max_size: Some(1141), added: 1636, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 19]`.
	fn set_account_id(r: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `121 + r * (57 ±0)`
		//  Estimated: `1636`
		// Minimum execution time: 7_292 nanoseconds.
		Weight::from_ref_time(7_744_432)
			.saturating_add(Weight::from_proof_size(1636))
			// Standard Error: 1_228
			.saturating_add(Weight::from_ref_time(103_386).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity Registrars (r:1 w:1)
	/// Proof: Identity Registrars (max_values: Some(1), max_size: Some(1141), added: 1636, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 19]`.
	fn set_fields(r: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `121 + r * (57 ±0)`
		//  Estimated: `1636`
		// Minimum execution time: 7_335 nanoseconds.
		Weight::from_ref_time(7_713_621)
			.saturating_add(Weight::from_proof_size(1636))
			// Standard Error: 1_281
			.saturating_add(Weight::from_ref_time(99_586).saturating_mul(r.into()))
			.saturating_add(T::DbWeight::get().reads(1))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity Registrars (r:1 w:0)
	/// Proof: Identity Registrars (max_values: Some(1), max_size: Some(1141), added: 1636, mode: MaxEncodedLen)
	/// Storage: Identity IdentityOf (r:1 w:1)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 19]`.
	/// The range of component `x` is `[0, 100]`.
	fn provide_judgement(r: u32, x: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `509 + r * (57 ±0) + x * (66 ±0)`
		//  Estimated: `11649`
		// Minimum execution time: 23_161 nanoseconds.
		Weight::from_ref_time(22_446_907)
			.saturating_add(Weight::from_proof_size(11649))
			// Standard Error: 5_391
			.saturating_add(Weight::from_ref_time(112_497).saturating_mul(r.into()))
			// Standard Error: 997
			.saturating_add(Weight::from_ref_time(769_392).saturating_mul(x.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// Storage: Identity IdentityOf (r:1 w:1)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: System Account (r:1 w:1)
	/// Proof: System Account (max_values: None, max_size: Some(128), added: 2603, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:0 w:100)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// The range of component `r` is `[1, 20]`.
	/// The range of component `s` is `[0, 100]`.
	/// The range of component `x` is `[0, 100]`.
	fn kill_identity(r: u32, s: u32, x: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `772 + r * (5 ±0) + s * (32 ±0) + x * (66 ±0)`
		//  Estimated: `18349`
		// Minimum execution time: 54_015 nanoseconds.
		Weight::from_ref_time(32_925_712)
			.saturating_add(Weight::from_proof_size(18349))
			// Standard Error: 7_533
			.saturating_add(Weight::from_ref_time(63_262).saturating_mul(r.into()))
			// Standard Error: 1_471
			.saturating_add(Weight::from_ref_time(1_155_232).saturating_mul(s.into()))
			// Standard Error: 1_471
			.saturating_add(Weight::from_ref_time(226_801).saturating_mul(x.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(3))
			.saturating_add(T::DbWeight::get().writes((1_u64).saturating_mul(s.into())))
	}
	/// Storage: Identity IdentityOf (r:1 w:0)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:1 w:1)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 99]`.
	fn add_sub(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `507 + s * (36 ±0)`
		//  Estimated: `18335`
		// Minimum execution time: 26_638 nanoseconds.
		Weight::from_ref_time(31_911_936)
			.saturating_add(Weight::from_proof_size(18335))
			// Standard Error: 1_403
			.saturating_add(Weight::from_ref_time(68_428).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Identity IdentityOf (r:1 w:0)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:1 w:1)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// The range of component `s` is `[1, 100]`.
	fn rename_sub(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `623 + s * (3 ±0)`
		//  Estimated: `12602`
		// Minimum execution time: 12_649 nanoseconds.
		Weight::from_ref_time(14_714_076)
			.saturating_add(Weight::from_proof_size(12602))
			// Standard Error: 739
			.saturating_add(Weight::from_ref_time(15_226).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(1))
	}
	/// Storage: Identity IdentityOf (r:1 w:0)
	/// Proof: Identity IdentityOf (max_values: None, max_size: Some(7538), added: 10013, mode: MaxEncodedLen)
	/// Storage: Identity SuperOf (r:1 w:1)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// The range of component `s` is `[1, 100]`.
	fn remove_sub(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `702 + s * (35 ±0)`
		//  Estimated: `18335`
		// Minimum execution time: 30_152 nanoseconds.
		Weight::from_ref_time(33_576_799)
			.saturating_add(Weight::from_proof_size(18335))
			// Standard Error: 937
			.saturating_add(Weight::from_ref_time(52_181).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(3))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	/// Storage: Identity SuperOf (r:1 w:1)
	/// Proof: Identity SuperOf (max_values: None, max_size: Some(114), added: 2589, mode: MaxEncodedLen)
	/// Storage: Identity SubsOf (r:1 w:1)
	/// Proof: Identity SubsOf (max_values: None, max_size: Some(3258), added: 5733, mode: MaxEncodedLen)
	/// The range of component `s` is `[0, 99]`.
	fn quit_sub(s: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `628 + s * (37 ±0)`
		//  Estimated: `8322`
		// Minimum execution time: 19_192 nanoseconds.
		Weight::from_ref_time(21_910_906)
			.saturating_add(Weight::from_proof_size(8322))
			// Standard Error: 1_053
			.saturating_add(Weight::from_ref_time(53_639).saturating_mul(s.into()))
			.saturating_add(T::DbWeight::get().reads(2))
			.saturating_add(T::DbWeight::get().writes(2))
	}
}
