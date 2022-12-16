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
//! Autogenerated weights for `pallet_xcm`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-12-16, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm3`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// /home/benchbot/cargo_target_dir/production/polkadot
// benchmark
// pallet
// --steps=50
// --repeat=20
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --heap-pages=4096
// --json-file=/var/lib/gitlab-runner/builds/zyw4fam_/0/parity/mirrors/polkadot/.git/.artifacts/bench.json
// --pallet=pallet_xcm
// --chain=westend-dev
// --header=./file_header.txt
// --output=./runtime/westend/src/weights/

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_xcm`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_xcm::WeightInfo for WeightInfo<T> {
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	fn send() -> Weight {
		// Minimum execution time: 37_493 nanoseconds.
		Weight::from_ref_time(38_370_000)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	fn teleport_assets() -> Weight {
		// Minimum execution time: 28_902 nanoseconds.
		Weight::from_ref_time(29_804_000)
	}
	fn reserve_transfer_assets() -> Weight {
		// Minimum execution time: 28_202 nanoseconds.
		Weight::from_ref_time(28_792_000)
	}
	// Storage: Benchmark Override (r:0 w:0)
	fn execute() -> Weight {
		// Minimum execution time: 18_446_744_073_709_551 nanoseconds.
		Weight::from_ref_time(18_446_744_073_709_551_000)
	}
	// Storage: XcmPallet SupportedVersion (r:0 w:1)
	fn force_xcm_version() -> Weight {
		// Minimum execution time: 15_311 nanoseconds.
		Weight::from_ref_time(15_701_000)
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: XcmPallet SafeXcmVersion (r:0 w:1)
	fn force_default_xcm_version() -> Weight {
		// Minimum execution time: 4_653 nanoseconds.
		Weight::from_ref_time(4_870_000)
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: XcmPallet VersionNotifiers (r:1 w:1)
	// Storage: XcmPallet QueryCounter (r:1 w:1)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	// Storage: XcmPallet Queries (r:0 w:1)
	fn force_subscribe_version_notify() -> Weight {
		// Minimum execution time: 41_951 nanoseconds.
		Weight::from_ref_time(42_918_000)
			.saturating_add(T::DbWeight::get().reads(7))
			.saturating_add(T::DbWeight::get().writes(6))
	}
	// Storage: XcmPallet VersionNotifiers (r:1 w:1)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	// Storage: XcmPallet Queries (r:0 w:1)
	fn force_unsubscribe_version_notify() -> Weight {
		// Minimum execution time: 45_280 nanoseconds.
		Weight::from_ref_time(45_907_000)
			.saturating_add(T::DbWeight::get().reads(6))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	// Storage: XcmPallet SupportedVersion (r:4 w:2)
	fn migrate_supported_version() -> Weight {
		// Minimum execution time: 15_016 nanoseconds.
		Weight::from_ref_time(15_334_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: XcmPallet VersionNotifiers (r:4 w:2)
	fn migrate_version_notifiers() -> Weight {
		// Minimum execution time: 15_103 nanoseconds.
		Weight::from_ref_time(15_434_000)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:5 w:0)
	fn already_notified_target() -> Weight {
		// Minimum execution time: 17_093 nanoseconds.
		Weight::from_ref_time(17_308_000)
			.saturating_add(T::DbWeight::get().reads(5))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:2 w:1)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	fn notify_current_targets() -> Weight {
		// Minimum execution time: 37_837 nanoseconds.
		Weight::from_ref_time(38_444_000)
			.saturating_add(T::DbWeight::get().reads(7))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:3 w:0)
	fn notify_target_migration_fail() -> Weight {
		// Minimum execution time: 7_623 nanoseconds.
		Weight::from_ref_time(7_768_000)
			.saturating_add(T::DbWeight::get().reads(3))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:3 w:0)
	fn migrate_version_notify_targets() -> Weight {
		// Minimum execution time: 7_391 nanoseconds.
		Weight::from_ref_time(7_604_000)
			.saturating_add(T::DbWeight::get().reads(3))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:3 w:0)
	fn migrate_and_notify_old_targets() -> Weight {
		// Minimum execution time: 7_578 nanoseconds.
		Weight::from_ref_time(7_824_000)
			.saturating_add(T::DbWeight::get().reads(3))
	}
}
