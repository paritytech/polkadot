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
impl proxy::WeightInfo for WeightInfo {
	fn proxy(p: u32, ) -> Weight {
		(27894000 as Weight)
			.saturating_add((196000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
	}
	fn announced_proxy(p: u32, ) -> Weight {
		(78353000 as Weight)
			.saturating_add((201000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn remove_announcement(p: u32, ) -> Weight {
		(60264000 as Weight)
			.saturating_add((4000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn reject_announcement(p: u32, ) -> Weight {
		(60141000 as Weight)
			.saturating_add((10000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn announce(p: u32, ) -> Weight {
		(74751000 as Weight)
			.saturating_add((208000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(3 as Weight))
			.saturating_add(DbWeight::get().writes(2 as Weight))
	}
	fn add_proxy(p: u32, ) -> Weight {
		(36250000 as Weight)
			.saturating_add((223000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn remove_proxy(p: u32, ) -> Weight {
		(32843000 as Weight)
			.saturating_add((258000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn remove_proxies(p: u32, ) -> Weight {
		(31834000 as Weight)
			.saturating_add((203000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn anonymous(p: u32, ) -> Weight {
		(50947000 as Weight)
			.saturating_add((44000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(2 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
	fn kill_anonymous(p: u32, ) -> Weight {
		(34213000 as Weight)
			.saturating_add((208000 as Weight).saturating_mul(p as Weight))
			.saturating_add(DbWeight::get().reads(1 as Weight))
			.saturating_add(DbWeight::get().writes(1 as Weight))
	}
}
