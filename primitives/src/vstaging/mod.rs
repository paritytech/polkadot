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

//! Staging Primitives.

use parity_scale_codec::{Decode, Encode};

pub use crate::v2::*;

sp_api::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	// In the staging API, this is u32::MAX.
	#[api_version(4294967295)]
	pub trait ParachainHost<H: Encode + Decode = Hash, N: Encode + Decode = BlockNumber> {}
}
