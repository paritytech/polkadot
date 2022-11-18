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

//! Abstract execution environment parameter set.
//!
//! Parameter set is encoded as an opaque vector which structure depends on the execution
//! environment itself (except for environment type/version which is always represented
//! by the first element of the vector). Decoding to a usable semantics structure is
//! done in `polkadot-node-core-pvf`.

use crate::v2::{BlakeTwo256, HashT as _};
use parity_scale_codec::{Decode, Encode};
#[cfg(feature = "std")]
use parity_util_mem::MallocSizeOf;
use polkadot_core_primitives::Hash;
use scale_info::TypeInfo;
use sp_std::{ops::Deref, vec::Vec};

/// # Execution environment parameter tags
/// Values from 1 to 16 are reserved for system-wide parameters not related to any concrete
/// execution environment.
/// Environment type
pub const EEPAR_01_ENVIRONMENT: u32 = 1;
/// Logical stack limit, in stack items
pub const EEPAR_17_STACK_LOGICAL_MAX: u32 = 17;
/// Native stack limit, in bytes
pub const EEPAR_18_STACK_NATIVE_MAX: u32 = 18;

/// # Execution enviroment types
/// Generic Wasmtime environment
pub const EXEC_ENV_TYPE_WASMTIME_GENERIC: u32 = 0;

/// Unit type wrapper around [`type@Hash`] that represents an execution parameter set hash.
///
/// This type is produced by [`ExecutorParams::hash`].
#[derive(Clone, Copy, Encode, Decode, Hash, Eq, PartialEq, PartialOrd, Ord, TypeInfo)]
pub struct ExecutorParamsHash(Hash);

impl sp_std::fmt::Display for ExecutorParamsHash {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter<'_>) -> sp_std::fmt::Result {
		self.0.fmt(f)
	}
}

impl sp_std::fmt::Debug for ExecutorParamsHash {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter<'_>) -> sp_std::fmt::Result {
		write!(f, "{:?}", self.0)
	}
}

impl sp_std::fmt::LowerHex for ExecutorParamsHash {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter<'_>) -> sp_std::fmt::Result {
		sp_std::fmt::LowerHex::fmt(&self.0, f)
	}
}

impl From<Hash> for ExecutorParamsHash {
	fn from(hash: Hash) -> Self {
		Self(hash)
	}
}

impl From<[u8; 32]> for ExecutorParamsHash {
	fn from(hash: [u8; 32]) -> Self {
		Self(hash.into())
	}
}

/// # Deterministically serialized execution environment semantics
/// Represents an arbitrary semantics of an arbitrary execution environment, so should be kept as
/// abstract as possible. Mapping from `u32` constant tags to SCALE-encoded values was chosen
/// over an idiomatic `enum` representation as the latter would require caller to rely on
/// deserialization errors when decoding a structure of newer version not yet known to the node
/// software, which is undesirable.
/// As this one goes on-chain, it requires full serialization determinism. That is ensured by
/// enforcing constant tags to be in ascending order and using deterministic SCALE-codec for values.
#[cfg_attr(feature = "std", derive(MallocSizeOf))]
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub struct ExecutorParams(Vec<(u32, Vec<u8>)>);

impl ExecutorParams {
	/// Creates a new, empty executor parameter set
	pub fn new() -> Self {
		ExecutorParams(Vec::new())
	}

	/// Returns execution environment type identifier
	pub fn environment(&self) -> u32 {
		if self.0.len() < 1 || self.0[0].0 != EEPAR_01_ENVIRONMENT {
			return EXEC_ENV_TYPE_WASMTIME_GENERIC
		}
		let env_enc = self.0[0].1.clone();
		match u32::decode(&mut &env_enc[..]) {
			Ok(env) => env,
			_ => EXEC_ENV_TYPE_WASMTIME_GENERIC,
		}
	}

	/// Adds an execution parameter to the set
	pub fn add(&mut self, tag: u32, value: impl Encode) {
		// Ensure deterministic order of tags
		#[cfg(debug_assertions)]
		if self.0.len() > 0 {
			let (last_tag, _) = self.0[self.0.len() - 1];
			debug_assert!(tag > last_tag);
		}

		self.0.push((tag, value.encode()));
	}

	/// Returns hash of the set of execution environment parameters
	pub fn hash(&self) -> ExecutorParamsHash {
		ExecutorParamsHash(BlakeTwo256::hash(&self.encode()))
	}
}

impl Deref for ExecutorParams {
	type Target = Vec<(u32, Vec<u8>)>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl Default for ExecutorParams {
	fn default() -> Self {
		ExecutorParams(Vec::new())
	}
}
