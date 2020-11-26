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

use sp_std::{result, ops::Add, convert::{TryFrom, TryInto}};
use sp_runtime::traits::CheckedSub;
use parity_scale_codec::{Encode, Decode};

/// Total number of possible sub ranges of slots.
pub const SLOT_RANGE_COUNT: usize = 10;

/// A compactly represented sub-range from the series (0, 1, 2, 3).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode)]
#[repr(u8)]
pub enum SlotRange {
	/// Sub range from index 0 to index 0 inclusive.
	ZeroZero = 0,
	/// Sub range from index 0 to index 1 inclusive.
	ZeroOne = 1,
	/// Sub range from index 0 to index 2 inclusive.
	ZeroTwo = 2,
	/// Sub range from index 0 to index 3 inclusive.
	ZeroThree = 3,
	/// Sub range from index 1 to index 1 inclusive.
	OneOne = 4,
	/// Sub range from index 1 to index 2 inclusive.
	OneTwo = 5,
	/// Sub range from index 1 to index 3 inclusive.
	OneThree = 6,
	/// Sub range from index 2 to index 2 inclusive.
	TwoTwo = 7,
	/// Sub range from index 2 to index 3 inclusive.
	TwoThree = 8,
	/// Sub range from index 3 to index 3 inclusive.
	ThreeThree = 9,     // == SLOT_RANGE_COUNT - 1
}

#[cfg(feature = "std")]
impl std::fmt::Debug for SlotRange {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		let p = self.as_pair();
		write!(fmt, "[{}..{}]", p.0, p.1)
	}
}

impl SlotRange {
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
		match first {
			0 => match count {
				0 => Some(SlotRange::ZeroZero),
				1 => Some(SlotRange::ZeroOne),
				2 => Some(SlotRange::ZeroTwo),
				3 => Some(SlotRange::ZeroThree),
				_ => None,
			},
			1 => match count {
				0 => Some(SlotRange::OneOne),
				1 => Some(SlotRange::OneTwo),
				2 => Some(SlotRange::OneThree),
				_ => None
			},
			2 => match count { 0 => Some(SlotRange::TwoTwo), 1 => Some(SlotRange::TwoThree), _ => None },
			3 => match count { 0 => Some(SlotRange::ThreeThree), _ => None },
			_ => return Err("range begins too late"),
		}.ok_or("range ends too late")
	}

	pub fn as_pair(&self) -> (u8, u8) {
		match self {
			SlotRange::ZeroZero => (0, 0),
			SlotRange::ZeroOne => (0, 1),
			SlotRange::ZeroTwo => (0, 2),
			SlotRange::ZeroThree => (0, 3),
			SlotRange::OneOne => (1, 1),
			SlotRange::OneTwo => (1, 2),
			SlotRange::OneThree => (1, 3),
			SlotRange::TwoTwo => (2, 2),
			SlotRange::TwoThree => (2, 3),
			SlotRange::ThreeThree => (3, 3),
		}
	}

	pub fn intersects(&self, other: SlotRange) -> bool {
		let a = self.as_pair();
		let b = other.as_pair();
		b.0 <= a.1 && a.0 <= b.1
//		== !(b.0 > a.1 || a.0 > b.1)
	}

	pub fn len(&self) -> usize {
		match self {
			SlotRange::ZeroZero => 1,
			SlotRange::ZeroOne => 2,
			SlotRange::ZeroTwo => 3,
			SlotRange::ZeroThree => 4,
			SlotRange::OneOne => 1,
			SlotRange::OneTwo => 2,
			SlotRange::OneThree => 3,
			SlotRange::TwoTwo => 1,
			SlotRange::TwoThree => 2,
			SlotRange::ThreeThree => 1,
		}
	}
}

impl TryFrom<usize> for SlotRange {
	type Error = ();
	fn try_from(x: usize) -> Result<SlotRange, ()> {
		Ok(match x {
			0 => SlotRange::ZeroZero,
			1 => SlotRange::ZeroOne,
			2 => SlotRange::ZeroTwo,
			3 => SlotRange::ZeroThree,
			4 => SlotRange::OneOne,
			5 => SlotRange::OneTwo,
			6 => SlotRange::OneThree,
			7 => SlotRange::TwoTwo,
			8 => SlotRange::TwoThree,
			9 => SlotRange::ThreeThree,
			_ => return Err(()),
		})
	}
}
