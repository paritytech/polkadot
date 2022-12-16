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
//! DATE: 2022-11-08, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm3`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

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
// --chain=kusama-dev
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/

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
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	fn send() -> Weight {
		// Minimum execution time: 32_807 nanoseconds.
		Weight::from_ref_time(33_383_000 as u64)
			.saturating_add(T::DbWeight::get().reads(5 as u64))
			.saturating_add(T::DbWeight::get().writes(3 as u64))
	}
	fn teleport_assets() -> Weight {
		// Minimum execution time: 28_036 nanoseconds.
		Weight::from_ref_time(28_386_000 as u64)
	}
	fn reserve_transfer_assets() -> Weight {
		// Minimum execution time: 26_239 nanoseconds.
		Weight::from_ref_time(27_146_000 as u64)
	}
	fn execute() -> Weight {
		// Minimum execution time: 14_327 nanoseconds.
		Weight::from_ref_time(14_693_000 as u64)
	}
	// Storage: XcmPallet SupportedVersion (r:0 w:1)
	fn force_xcm_version() -> Weight {
		// Minimum execution time: 13_854 nanoseconds.
		Weight::from_ref_time(14_658_000 as u64)
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: XcmPallet SafeXcmVersion (r:0 w:1)
	fn force_default_xcm_version() -> Weight {
		// Minimum execution time: 3_786 nanoseconds.
		Weight::from_ref_time(3_986_000 as u64)
			.saturating_add(T::DbWeight::get().writes(1 as u64))
	}
	// Storage: XcmPallet VersionNotifiers (r:1 w:1)
	// Storage: XcmPallet QueryCounter (r:1 w:1)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: XcmPallet Queries (r:0 w:1)
	fn force_subscribe_version_notify() -> Weight {
		// Minimum execution time: 37_959 nanoseconds.
		Weight::from_ref_time(38_689_000 as u64)
			.saturating_add(T::DbWeight::get().reads(7 as u64))
			.saturating_add(T::DbWeight::get().writes(6 as u64))
	}
	// Storage: XcmPallet VersionNotifiers (r:1 w:1)
	// Storage: XcmPallet SupportedVersion (r:1 w:0)
	// Storage: XcmPallet VersionDiscoveryQueue (r:1 w:1)
	// Storage: XcmPallet SafeXcmVersion (r:1 w:0)
	// Storage: Dmp DownwardMessageQueueHeads (r:1 w:1)
	// Storage: Dmp DownwardMessageQueues (r:1 w:1)
	// Storage: XcmPallet Queries (r:0 w:1)
	fn force_unsubscribe_version_notify() -> Weight {
		// Minimum execution time: 41_213 nanoseconds.
		Weight::from_ref_time(41_668_000 as u64)
			.saturating_add(T::DbWeight::get().reads(6 as u64))
			.saturating_add(T::DbWeight::get().writes(5 as u64))
	}
	fn migrate_supported_version() -> Weight {
		Weight::from_ref_time(100_000_000)
	}

	fn migrate_version_notifiers() -> Weight {
		Weight::from_ref_time(100_000_000)
	}

	fn already_notified_target() -> Weight {
		Weight::from_ref_time(100_000_000)
	}

	fn notify_current_targets() -> Weight {
		Weight::from_ref_time(100_000_000)
	}

	fn notify_target_migration_fail() -> Weight {
		Weight::from_ref_time(100_000_000)
	}

	fn migrate_version_notify_targets() -> Weight {
		Weight::from_ref_time(100_000_000)
	}

	fn migrate_and_notify_old_targets() -> Weight {
		Weight::from_ref_time(100_000_000)
	}
}
