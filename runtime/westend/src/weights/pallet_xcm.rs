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
//! DATE: 2023-01-23, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `runner-b3zmxxc-project-163-concurrent-0`, CPU: `Intel(R) Xeon(R) CPU @ 2.60GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("westend-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=westend-dev
// --steps=50
// --repeat=20
// --pallet=pallet_xcm
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

/// Weight functions for `pallet_xcm`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_xcm::WeightInfo for WeightInfo<T> {
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	fn send() -> Weight {
		// Minimum execution time: 38_435 nanoseconds.
		Weight::from_parts(39_264_000, 0)
			.saturating_add(T::DbWeight::get().reads(5))
			.saturating_add(T::DbWeight::get().writes(3))
	}
	fn teleport_assets() -> Weight {
		// Minimum execution time: 29_090 nanoseconds.
		Weight::from_parts(29_801_000, 0)
	}
	fn reserve_transfer_assets() -> Weight {
		// Minimum execution time: 27_998 nanoseconds.
		Weight::from_parts(29_079_000, 0)
	}
	// Storage: Benchmark Override (r:0 w:0)
	fn execute() -> Weight {
		// Minimum execution time: 18_446_744_073_709_551 nanoseconds.
		Weight::from_parts(18_446_744_073_709_551_000, 0)
	}
	// Storage: XcmPallet SupportedVersion (r:0 w:1)
	fn force_xcm_version() -> Weight {
		// Minimum execution time: 15_893 nanoseconds.
		Weight::from_parts(16_196_000, 0)
			.saturating_add(T::DbWeight::get().writes(1))
	}
	// Storage: XcmPallet SafeXcmVersion (r:0 w:1)
	fn force_default_xcm_version() -> Weight {
		// Minimum execution time: 4_403 nanoseconds.
		Weight::from_parts(4_553_000, 0)
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
		// Minimum execution time: 42_466 nanoseconds.
		Weight::from_parts(43_907_000, 0)
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
		// Minimum execution time: 46_063 nanoseconds.
		Weight::from_parts(47_278_000, 0)
			.saturating_add(T::DbWeight::get().reads(6))
			.saturating_add(T::DbWeight::get().writes(5))
	}
	// Storage: XcmPallet SupportedVersion (r:4 w:2)
	fn migrate_supported_version() -> Weight {
		// Minimum execution time: 15_315 nanoseconds.
		Weight::from_parts(15_983_000, 0)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: XcmPallet VersionNotifiers (r:4 w:2)
	fn migrate_version_notifiers() -> Weight {
		// Minimum execution time: 15_715 nanoseconds.
		Weight::from_parts(16_212_000, 0)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:5 w:0)
	fn already_notified_target() -> Weight {
		// Minimum execution time: 18_319 nanoseconds.
		Weight::from_parts(18_821_000, 0)
			.saturating_add(T::DbWeight::get().reads(5))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:2 w:1)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	fn notify_current_targets() -> Weight {
		// Minimum execution time: 38_303 nanoseconds.
		Weight::from_parts(39_210_000, 0)
			.saturating_add(T::DbWeight::get().reads(7))
			.saturating_add(T::DbWeight::get().writes(4))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:3 w:0)
	fn notify_target_migration_fail() -> Weight {
		// Minimum execution time: 8_105 nanoseconds.
		Weight::from_parts(8_402_000, 0)
			.saturating_add(T::DbWeight::get().reads(3))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:4 w:2)
	fn migrate_version_notify_targets() -> Weight {
		// Minimum execution time: 16_035 nanoseconds.
		Weight::from_parts(16_561_000, 0)
			.saturating_add(T::DbWeight::get().reads(4))
			.saturating_add(T::DbWeight::get().writes(2))
	}
	// Storage: XcmPallet VersionNotifyTargets (r:4 w:2)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	fn migrate_and_notify_old_targets() -> Weight {
		// Minimum execution time: 45_300 nanoseconds.
		Weight::from_parts(46_946_000, 0)
			.saturating_add(T::DbWeight::get().reads(9))
			.saturating_add(T::DbWeight::get().writes(5))
	}
}
