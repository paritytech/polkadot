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

use sp_std::{ops::Deref, vec::Vec};
// use sp_core::Hash;
use parity_scale_codec::{Decode, Encode};
use parity_util_mem::MallocSizeOf;
use scale_info::TypeInfo;

/// Deterministically serialized execution environment semantics
/// to include into `SessionInfo`
#[derive(Clone, Debug, Encode, Decode, PartialEq, MallocSizeOf, TypeInfo)]
pub struct ExecutorParams(Vec<(u8, u64)>);

impl ExecutorParams {
	pub fn version(&self) -> u64 {
		if self.0.len() == 0 {
			0
		} else {
			// EEPAR_VERSION must always be the first element of ExecutorParams
			self.0[0].1
		}
	}
}

impl Deref for ExecutorParams {
	type Target = Vec<(u8, u64)>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl Default for ExecutorParams {
	fn default() -> Self {
		ExecutorParams(Vec::new())
	}
}
