// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use parity_util_mem::MallocSizeOf;
use sp_runtime::traits::CheckEqual;

// `sp_core::H512` can't be used, because it doesn't implement `CheckEqual`, which is required
// by `frame_system::Config::Hash`.

fixed_hash::construct_fixed_hash! {
	/// Hash type used in Millau chain.
	#[derive(MallocSizeOf)]
	pub struct MillauHash(64);
}

#[cfg(feature = "std")]
impl_serde::impl_fixed_hash_serde!(MillauHash, 64);

impl_codec::impl_fixed_hash_codec!(MillauHash, 64);

impl CheckEqual for MillauHash {
	#[cfg(feature = "std")]
	fn check_equal(&self, other: &Self) {
		use sp_core::hexdisplay::HexDisplay;
		if self != other {
			println!(
				"Hash: given={}, expected={}",
				HexDisplay::from(self.as_fixed_bytes()),
				HexDisplay::from(other.as_fixed_bytes()),
			);
		}
	}

	#[cfg(not(feature = "std"))]
	fn check_equal(&self, other: &Self) {
		use frame_support::Printable;

		if self != other {
			"Hash not equal".print();
			self.as_bytes().print();
			other.as_bytes().print();
		}
	}
}
