// Copyright 2019 Parity Technologies (UK) Ltd.
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

/// Money matters.
pub mod currency {
	use primitives::Balance;

	pub const DOTS: Balance = 1_000_000_000_000;
	pub const BUCKS: Balance = DOTS / 100;
	pub const CENTS: Balance = BUCKS / 100;
	pub const MILLICENTS: Balance = CENTS / 1_000;
}

/// Time.
pub mod time {
	pub const SECS_PER_BLOCK: u64 = 6;
	pub const MINUTES: u64 = 60 / SECS_PER_BLOCK;
	pub const HOURS: u64 = MINUTES * 60;
	pub const DAYS: u64 = HOURS * 24;
}

/// Fee-related.
pub mod fee {
	pub use sr_primitives::Perbill;

	/// The block saturation level. Fees will be updates based on this value.
	pub const TARGET_BLOCK_FULLNESS: Perbill = Perbill::from_percent(25);
}
