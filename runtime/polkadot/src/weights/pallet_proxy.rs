// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 2.0.0-rc5

use frame_support::weights::{Weight, constants::RocksDbWeight as DbWeight};

pub struct WeightInfo;
impl pallet_proxy::WeightInfo for WeightInfo {
	fn proxy(p: u32, ) -> Weight {
		(26127000 as Weight)
			.saturating_add((214000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
	}
	fn proxy_announced(a: u32, p: u32, ) -> Weight {
		(55405000 as Weight)
			.saturating_add((774000 as Weight).saturating_mul(a as Weight))
			.saturating_add((209000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn remove_announcement(a: u32, p: u32, ) -> Weight {
		(35879000 as Weight)
			.saturating_add((783000 as Weight).saturating_mul(a as Weight))
			.saturating_add((20000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn reject_announcement(a: u32, p: u32, ) -> Weight {
		(36097000 as Weight)
			.saturating_add((780000 as Weight).saturating_mul(a as Weight))
			.saturating_add((12000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn announce(a: u32, p: u32, ) -> Weight {
		(53769000 as Weight)
			.saturating_add((675000 as Weight).saturating_mul(a as Weight))
			.saturating_add((214000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn add_proxy(p: u32, ) -> Weight {
		(36082000 as Weight)
			.saturating_add((234000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn remove_proxy(p: u32, ) -> Weight {
		(32885000 as Weight)
			.saturating_add((267000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn remove_proxies(p: u32, ) -> Weight {
		(31735000 as Weight)
			.saturating_add((215000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn anonymous(p: u32, ) -> Weight {
		(50907000 as Weight)
			.saturating_add((61000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn kill_anonymous(p: u32, ) -> Weight {
		(33926000 as Weight)
			.saturating_add((208000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
}
