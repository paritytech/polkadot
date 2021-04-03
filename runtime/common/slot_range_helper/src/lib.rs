// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! A helper macro for generating SlotRange enum.

#![cfg_attr(not(feature = "std"), no_std)]

pub use sp_std::{result, ops::Add, convert::TryInto};
pub use sp_runtime::traits::CheckedSub;
pub use parity_scale_codec::{Encode, Decode};
pub use paste;

/// This macro generates a `SlotRange` enum of arbitrary length for use in the Slot Auction
/// mechanism on Polkadot.
///
/// Usage:
/// ```
/// slot_range_helper::generate_slot_range!(Zero(0), One(1), Two(2), Three(3));
/// ```
///
/// To extend the usage, continue to add `Identifier(value)` items to the macro.
///
/// This will generate an enum `SlotRange` with the following properties:
///
/// * Enum variants will range from all consecutive combinations of inputs, i.e.
///   `ZeroZero`, `ZeroOne`, `ZeroTwo`, `ZeroThree`, `OneOne`, `OneTwo`, `OneThree`...
/// * A constant `LEASE_PERIODS_PER_SLOT` will count the number of lease periods.
/// * A constant `SLOT_RANGE_COUNT` will count the total number of enum variants.
/// * A function `as_pair` will return a tuple representation of the `SlotRange`.
/// * A function `intersects` will tell you if two slot ranges intersect with one another.
/// * A function `len` will tell you the length of occupying a `SlotRange`.
/// * A function `new_bounded` will generate a `SlotRange` from an input of the current
///   lease period, the starting lease period, and the final lease period.
#[macro_export]
macro_rules! generate_slot_range{
	// Entry point
	($( $x:ident ( $e:expr ) ),*) => {
		$crate::generate_lease_period_per_slot!( $( $x )* );
		$crate::generate_slot_range!(@inner
			{ }
			$( $x ( $e ) )*
		);
	};
	// Does the magic...
	(@inner
		{ $( $parsed:ident ( $t1:expr, $t2:expr ) )* }
		$current:ident ( $ce:expr )
		$( $remaining:ident ( $re:expr ) )*
	) => {
		$crate::paste::paste! {
			$crate::generate_slot_range!(@inner
				{
					$( $parsed ( $t1, $t2 ) )*
					[< $current $current >] ( $ce, $ce )
					$( [< $current $remaining >] ($ce, $re) )*
				}
				$( $remaining ( $re ) )*
			);
		}
	};
	(@inner
		{ $( $parsed:ident ( $t1:expr, $t2:expr ) )* }
	) => {
		$crate::generate_slot_range_enum!(@inner $( $parsed )* );

		$crate::generate_slot_range_count!( $( $parsed )* );

		#[cfg(feature = "std")]
		impl std::fmt::Debug for SlotRange {
			fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
				let p = self.as_pair();
				write!(fmt, "[{}..{}]", p.0, p.1)
			}
		}

		impl SlotRange {
			pub const LEASE_PERIODS_PER_SLOT: usize = LEASE_PERIODS_PER_SLOT;
			pub const SLOT_RANGE_COUNT: usize = SLOT_RANGE_COUNT;

			$crate::generate_slot_range_as_pair!(@inner $( $parsed ( $t1, $t2 ) )* );

			$crate::generate_slot_range_len!(@inner $( $parsed ( $t1, $t2 ) )* );

			$crate::generate_slot_range_new_bounded!(@inner $( $parsed ( $t1, $t2 ) )* );
		}
	};
}

#[macro_export]
#[doc(hidden)]
macro_rules! generate_slot_range_enum {
	(@inner
		$( $parsed:ident )*
	) => {
		/// A compactly represented sub-range from the series.
		#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, $crate::Encode, $crate::Decode)]
		#[repr(u8)]
		pub enum SlotRange { $( $parsed ),* }
	};
}

#[macro_export]
#[doc(hidden)]
macro_rules! generate_slot_range_as_pair {
	(@inner
		$( $parsed:ident ( $t1:expr, $t2:expr ) )*
	) => {
		/// Return true if two `SlotRange` intersect in their lease periods.
		pub fn intersects(&self, other: SlotRange) -> bool {
			let a = self.as_pair();
			let b = other.as_pair();
			b.0 <= a.1 && a.0 <= b.1
			// == !(b.0 > a.1 || a.0 > b.1)
		}

		/// Return a tuple representation of the `SlotRange`.
		///
		/// Example:`SlotRange::OneTwo.as_pair() == (1, 2)`
		pub fn as_pair(&self) -> (u8, u8) {
			match self {
				$( SlotRange::$parsed => { ($t1, $t2) } )*
			}
		}
	};
}

#[macro_export]
#[doc(hidden)]
macro_rules! generate_slot_range_len {
	// Use evaluated length in function.
	(@inner
		$( $parsed:ident ( $t1:expr, $t2:expr ) )*
	) => {
		/// Return the length of occupying a `SlotRange`.
		///
		/// Example:`SlotRange::OneTwo.len() == 2`
		pub fn len(&self) -> usize {
			match self {
				// len (0, 2) = 2 - 0 + 1 = 3
				$( SlotRange::$parsed => { ( $t2 - $t1 + 1) } )*
			}
		}
	};
}

#[macro_export]
#[doc(hidden)]
macro_rules! generate_slot_range_new_bounded {
	(@inner
		$( $parsed:ident ( $t1:expr, $t2:expr ) )*
	) => {
		/// Construct a `SlotRange` from the current lease period, the first lease period of the range,
		/// and the last lease period of the range.
		///
		/// For example: `SlotRange::new_bounded(1, 2, 3) == SlotRange::OneTwo`.
		pub fn new_bounded<
			Index: $crate::Add<Output=Index> + $crate::CheckedSub + Copy + Ord + From<u32> + $crate::TryInto<u32>
		>(
			current: Index,
			first: Index,
			last: Index
		) -> $crate::result::Result<Self, &'static str> {
			if first > last || first < current || last >= current + (LEASE_PERIODS_PER_SLOT as u32).into() {
				return Err("Invalid range for this auction")
			}
			let count: u32 = last.checked_sub(&first)
				.ok_or("range ends before it begins")?
				.try_into()
				.map_err(|_| "range too big")?;
			let first: u32 = first.checked_sub(&current)
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

#[macro_export]
#[doc(hidden)]
macro_rules! generate_slot_range_count {
	(
		$start:ident $( $rest:ident )*
	) => {
		$crate::generate_slot_range_count!(@inner 1; $( $rest )*);
	};
	(@inner
		$count:expr;
		$start:ident $( $rest:ident )*
	) => {
		$crate::generate_slot_range_count!(@inner $count + 1; $( $rest )*);
	};
	(@inner
		$count:expr;
	) => {
		const SLOT_RANGE_COUNT: usize = $count;
	};
}

#[macro_export]
#[doc(hidden)]
macro_rules! generate_lease_period_per_slot {
	(
		$start:ident $( $rest:ident )*
	) => {
		$crate::generate_lease_period_per_slot!(@inner 1; $( $rest )*);
	};
	(@inner
		$count:expr;
		$start:ident $( $rest:ident )*
	) => {
		$crate::generate_lease_period_per_slot!(@inner $count + 1; $( $rest )*);
	};
	(@inner
		$count:expr;
	) => {
		const LEASE_PERIODS_PER_SLOT: usize = $count;
	};
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn slot_range_4_works() {
		generate_slot_range!(Zero(0), One(1), Two(2), Three(3));

		assert_eq!(SlotRange::LEASE_PERIODS_PER_SLOT, 4);
		// Sum over n from 0 - 4
		assert_eq!(SlotRange::SLOT_RANGE_COUNT, 10);
		assert_eq!(SlotRange::new_bounded(0u32, 1u32, 2u32).unwrap(), SlotRange::OneTwo);
		assert_eq!(SlotRange::new_bounded(5u32, 6u32, 7u32).unwrap(), SlotRange::OneTwo);
		assert!(SlotRange::new_bounded(10u32, 6u32, 7u32).is_err());
		assert!(SlotRange::new_bounded(10u32, 16u32, 17u32).is_err());
		assert!(SlotRange::new_bounded(10u32, 11u32, 10u32).is_err());
		assert_eq!(SlotRange::TwoTwo.len(), 1);
		assert_eq!(SlotRange::OneTwo.len(), 2);
		assert_eq!(SlotRange::ZeroThree.len(), 4);
		assert!(SlotRange::ZeroOne.intersects(SlotRange::OneThree));
		assert!(!SlotRange::ZeroOne.intersects(SlotRange::TwoThree));
		assert_eq!(SlotRange::ZeroZero.as_pair(), (0, 0));
		assert_eq!(SlotRange::OneThree.as_pair(), (1, 3));
	}

	#[test]
	fn slot_range_8_works() {
		generate_slot_range!(Zero(0), One(1), Two(2), Three(3), Four(4), Five(5), Six(6), Seven(7));

		assert_eq!(SlotRange::LEASE_PERIODS_PER_SLOT, 8);
		// Sum over n from 0 to 8
		assert_eq!(SlotRange::SLOT_RANGE_COUNT, 36);
		assert_eq!(SlotRange::new_bounded(0u32, 1u32, 2u32).unwrap(), SlotRange::OneTwo);
		assert_eq!(SlotRange::new_bounded(5u32, 6u32, 7u32).unwrap(), SlotRange::OneTwo);
		assert!(SlotRange::new_bounded(10u32, 6u32, 7u32).is_err());
		// This one passes with slot range 8
		assert_eq!(SlotRange::new_bounded(10u32, 16u32, 17u32).unwrap(), SlotRange::SixSeven);
		assert!(SlotRange::new_bounded(10u32, 17u32, 18u32).is_err());
		assert!(SlotRange::new_bounded(10u32, 20u32, 21u32).is_err());
		assert!(SlotRange::new_bounded(10u32, 11u32, 10u32).is_err());
		assert_eq!(SlotRange::TwoTwo.len(), 1);
		assert_eq!(SlotRange::OneTwo.len(), 2);
		assert_eq!(SlotRange::ZeroThree.len(), 4);
		assert_eq!(SlotRange::ZeroSeven.len(), 8);
		assert!(SlotRange::ZeroOne.intersects(SlotRange::OneThree));
		assert!(!SlotRange::ZeroOne.intersects(SlotRange::TwoThree));
		assert!(SlotRange::FiveSix.intersects(SlotRange::SixSeven));
		assert!(!SlotRange::ThreeFive.intersects(SlotRange::SixSeven));
		assert_eq!(SlotRange::ZeroZero.as_pair(), (0, 0));
		assert_eq!(SlotRange::OneThree.as_pair(), (1, 3));
		assert_eq!(SlotRange::SixSeven.as_pair(), (6, 7));
	}
}
