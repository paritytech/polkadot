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
//! Autogenerated weights for `pallet_proxy`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 4.0.0-dev
//! DATE: 2022-08-19, STEPS: `50`, REPEAT: 20, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! HOSTNAME: `bm4`, CPU: `Intel(R) Core(TM) i7-7700K CPU @ 4.20GHz`
//! EXECUTION: Some(Wasm), WASM-EXECUTION: Compiled, CHAIN: Some("kusama-dev"), DB CACHE: 1024

// Executed Command:
// ./target/production/polkadot
// benchmark
// pallet
// --chain=kusama-dev
// --steps=50
// --repeat=20
// --pallet=pallet_proxy
// --extrinsic=*
// --execution=wasm
// --wasm-execution=compiled
// --header=./file_header.txt
// --output=./runtime/kusama/src/weights/pallet_proxy.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::{RefTimeWeight, Weight}};
use sp_std::marker::PhantomData;

/// Weight functions for `pallet_proxy`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_proxy::WeightInfo for WeightInfo<T> {
	// Storage: Proxy Proxies (r:1 w:0)
	/// The range of component `p` is `[1, 31]`.
	fn proxy(p: u32, ) -> Weight {
		(17_720_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((70_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
	}
	// Storage: Proxy Proxies (r:1 w:0)
	// Storage: Proxy Announcements (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	/// The range of component `a` is `[0, 31]`.
	/// The range of component `p` is `[1, 31]`.
	fn proxy_announced(a: u32, p: u32, ) -> Weight {
		(33_323_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((187_000 as RefTimeWeight).scalar_saturating_mul(a as Weight))
			// Standard Error: 1_000
			.saturating_add((49_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
	}
	// Storage: Proxy Announcements (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	/// The range of component `a` is `[0, 31]`.
	/// The range of component `p` is `[1, 31]`.
	fn remove_announcement(a: u32, p: u32, ) -> Weight {
		(22_697_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((170_000 as RefTimeWeight).scalar_saturating_mul(a as Weight))
			// Standard Error: 1_000
			.saturating_add((12_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
	}
	// Storage: Proxy Announcements (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	/// The range of component `a` is `[0, 31]`.
	/// The range of component `p` is `[1, 31]`.
	fn reject_announcement(a: u32, p: u32, ) -> Weight {
		(22_575_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((177_000 as RefTimeWeight).scalar_saturating_mul(a as Weight))
			// Standard Error: 1_000
			.saturating_add((6_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
	}
	// Storage: Proxy Proxies (r:1 w:0)
	// Storage: Proxy Announcements (r:1 w:1)
	// Storage: System Account (r:1 w:1)
	/// The range of component `a` is `[0, 31]`.
	/// The range of component `p` is `[1, 31]`.
	fn announce(a: u32, p: u32, ) -> Weight {
		(30_349_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((168_000 as RefTimeWeight).scalar_saturating_mul(a as Weight))
			// Standard Error: 1_000
			.saturating_add((41_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(3 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(2 as RefTimeWeight))
	}
	// Storage: Proxy Proxies (r:1 w:1)
	/// The range of component `p` is `[1, 31]`.
	fn add_proxy(p: u32, ) -> Weight {
		(25_144_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((105_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Proxy Proxies (r:1 w:1)
	/// The range of component `p` is `[1, 31]`.
	fn remove_proxy(p: u32, ) -> Weight {
		(24_770_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((131_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Proxy Proxies (r:1 w:1)
	/// The range of component `p` is `[1, 31]`.
	fn remove_proxies(p: u32, ) -> Weight {
		(20_974_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((72_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: unknown [0x3a65787472696e7369635f696e646578] (r:1 w:0)
	// Storage: Proxy Proxies (r:1 w:1)
	/// The range of component `p` is `[1, 31]`.
	fn anonymous(p: u32, ) -> Weight {
		(28_658_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((30_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(2 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
	// Storage: Proxy Proxies (r:1 w:1)
	/// The range of component `p` is `[0, 30]`.
	fn kill_anonymous(p: u32, ) -> Weight {
		(22_082_000 as RefTimeWeight)
			// Standard Error: 1_000
			.saturating_add((56_000 as RefTimeWeight).scalar_saturating_mul(p as Weight))
			.saturating_add(T::DbWeight::get().reads(1 as RefTimeWeight))
			.saturating_add(T::DbWeight::get().writes(1 as RefTimeWeight))
	}
}
