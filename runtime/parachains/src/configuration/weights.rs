
//! Autogenerated weights for `runtime_parachains::configuration`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2021-09-17, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 128

// Executed Command:
// ./target/release/polkadot
// benchmark
// --chain
// westend-dev
// --execution=wasm
// --wasm-execution=compiled
// --pallet
// runtime_parachains::configuration
// --steps
// 50
// --repeat
// 20
// --raw
// --extrinsic
// *
// --output
// runtime/parachains/src/configuration/weights.rs


#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for runtime_parachains::configuration.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains::configuration::WeightInfo for WeightInfo<T> {
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:0)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_config_with_block_number() -> Weight {
		(10_419_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_config_with_u32() -> Weight {
		(15_119_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_config_with_option_u32() -> Weight {
		(15_225_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_config_with_weight() -> Weight {
		(15_265_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: Benchmark Override (r:0 w:0)
	fn set_hrmp_open_request_ttl() -> Weight {
		(2_000_000_000_000 as Weight)
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_config_with_balance() -> Weight {
		(15_192_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
}
