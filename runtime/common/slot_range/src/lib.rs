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

//! The SlotRange struct which succinctly handles the ten values that
//! represent all sub ranges between 0 and 3 inclusive.

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::{result, ops::Add, convert::TryInto};
use sp_runtime::traits::CheckedSub;
use parity_scale_codec::{Encode, Decode};

/// Total number of possible sub ranges of slots.
pub const SLOT_RANGE_COUNT: usize = 10;

/// A macro for generating an enum of slot ranges over arbitrary range sizes.
macro_rules! generate_slot_range{
	// Entry point
	($( $x:ident ( $e:expr ) ),*) => {
		generate_slot_range!(@
			{ }
			$( $x ( $e ) )*
		);
	};
	// Does the magic...
	(@
		{ $( $parsed:ident ( $t1:expr, $t2:expr ) )* }
		$current:ident ( $ce:expr )
		$( $remaining:ident ( $re:expr ) )*
	) => {
		paste::paste! {
			generate_slot_range!(@
				{
					$( $parsed ( $t1, $t2 ) )*
					[< $current $current >] ( $ce, $ce )
					$( [< $current $remaining >] ($ce, $re) )*
				}
				$( $remaining ( $re ) )*
			);
		}
	};
	(@
		{ $( $parsed:ident ( $t1:expr, $t2:expr ) )* }
	) => {
		generate_slot_range_enum!(@ $( $parsed )* );

		#[cfg(feature = "std")]
		impl std::fmt::Debug for SlotRange {
			fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
				let p = self.as_pair();
				write!(fmt, "[{}..{}]", p.0, p.1)
			}
		}

		impl SlotRange {
			generate_slot_range_as_pair!(@ $( $parsed ( $t1, $t2 ) )* );

			generate_slot_range_len!(@ $( $parsed ( $t1, $t2 ) )* );

			generate_slot_range_new_bounded!(@ $( $parsed ( $t1, $t2 ) )* );
		}
	};
}

macro_rules! generate_slot_range_enum {
	(@
		$( $parsed:ident )*
	) => {
		generate_slot_range_enum! {
			/// A compactly represented sub-range from the series.
			#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode)]
			#[repr(u8)]
			pub enum SlotRange { $( $parsed ),* }
		}
	};
	// A trick from StackOverflow to generate the final Enum object.
	($i:item) => { $i };
}

macro_rules! generate_slot_range_as_pair {
	(@
		$( $parsed:ident ( $t1:expr, $t2:expr ) )*
	) => {
		pub fn intersects(&self, other: SlotRange) -> bool {
			let a = self.as_pair();
			let b = other.as_pair();
			b.0 <= a.1 && a.0 <= b.1
		//		== !(b.0 > a.1 || a.0 > b.1)
		}

		pub fn as_pair(&self) -> (u8, u8) {
			match self {
				$( SlotRange::$parsed => { ($t1, $t2) } )*
			}
		}
	};
}

macro_rules! generate_slot_range_len {
	// Use evaluated length in function.
	(@
		$( $parsed:ident ( $t1:expr, $t2:expr ) )*
	) => {
			pub fn len(&self) -> usize {
			match self {
				// len (0, 2) = 2 - 0 + 1 = 3
				$( SlotRange::$parsed => { ( $t2 - $t1 + 1) } )*
			}
		}
	};
}

macro_rules! generate_slot_range_new_bounded {
	(@
		$( $parsed:ident ( $t1:expr, $t2:expr ) )*
	) => {
		pub fn new_bounded<
			Index: Add<Output=Index> + CheckedSub + Copy + Ord + From<u32> + TryInto<u32>
		>(
			initial: Index,
			first: Index,
			last: Index
		) -> result::Result<Self, &'static str> {
			if first > last || first < initial || last > initial + 3.into() {
				return Err("Invalid range for this auction")
			}
			let count: u32 = last.checked_sub(&first)
				.ok_or("range ends before it begins")?
				.try_into()
				.map_err(|_| "range too big")?;
			let first: u32 = first.checked_sub(&initial)
				.ok_or("range begins too early")?
				.try_into()
				.map_err(|_| "start too far")?;
			match (first, first + count) {
				$( ($t1, $t2) => { Ok(SlotRange::$parsed) })*
				_ => Err("bad range"),
			}
		}
	};
}

generate_slot_range!(Zero(0), One(1), Two(2), Three(3));
