
//! Autogenerated weights for `runtime_parachains::configuration`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2021-09-16, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
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
impl<T: frame_system::Config> super::WeightInfo for WeightInfo<T> {
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:0)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_validation_upgrade_frequency() -> Weight {
		(11_878_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:0)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_validation_upgrade_delay() -> Weight {
		(11_594_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_code_retention_period() -> Weight {
		(15_986_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_code_size() -> Weight {
		(15_996_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_pov_size() -> Weight {
		(15_940_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_head_data_size() -> Weight {
		(15_830_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_parathread_cores() -> Weight {
		(15_996_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_parathread_retries() -> Weight {
		(15_899_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_group_rotation_frequency() -> Weight {
		(16_499_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_chain_availability_period() -> Weight {
		(15_994_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_thread_availability_period() -> Weight {
		(15_920_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_scheduling_lookahead() -> Weight {
		(16_041_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_validators_per_core() -> Weight {
		(15_971_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_validators() -> Weight {
		(16_149_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_dispute_period() -> Weight {
		(15_923_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_dispute_post_conclusion_acceptance_period() -> Weight {
		(16_047_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_dispute_max_spam_slots() -> Weight {
		(16_031_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_dispute_conclusion_by_time_out_period() -> Weight {
		(16_084_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_no_show_slots() -> Weight {
		(16_074_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_n_delay_tranches() -> Weight {
		(16_141_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_zeroth_delay_tranche_width() -> Weight {
		(16_024_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_needed_approvals() -> Weight {
		(15_957_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_relay_vrf_modulo_samples() -> Weight {
		(15_988_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_upward_queue_count() -> Weight {
		(15_879_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_upward_queue_size() -> Weight {
		(15_869_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:0)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_downward_message_size() -> Weight {
		(11_492_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_ump_service_total_weight() -> Weight {
		(15_896_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_upward_message_size() -> Weight {
		(15_916_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_max_upward_message_num_per_candidate() -> Weight {
		(15_892_000 as Weight)
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
	fn set_hrmp_sender_deposit() -> Weight {
		(15_931_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_recipient_deposit() -> Weight {
		(15_986_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_channel_max_capacity() -> Weight {
		(15_966_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_channel_max_total_size() -> Weight {
		(15_949_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_max_parachain_inbound_channels() -> Weight {
		(15_916_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_max_parathread_inbound_channels() -> Weight {
		(15_871_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_channel_max_message_size() -> Weight {
		(16_317_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_max_parachain_outbound_channels() -> Weight {
		(15_931_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_max_parathread_outbound_channels() -> Weight {
		(15_960_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
	// Storage: ParasShared CurrentSessionIndex (r:1 w:0)
	// Storage: Configuration PendingConfig (r:1 w:1)
	// Storage: Configuration ActiveConfig (r:1 w:0)
	fn set_hrmp_max_message_num_per_candidate() -> Weight {
		(15_885_000 as Weight)
			.saturating_add(T::DbWeight::get().reads(3 as Weight))
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
}
