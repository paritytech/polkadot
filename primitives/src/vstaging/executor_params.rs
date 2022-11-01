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

use parity_scale_codec::{Decode, Encode};
use parity_util_mem::MallocSizeOf;
use scale_info::TypeInfo;
use sp_std::{ops::Deref, vec::Vec};

/// # Execution environment parameter tags
/// Environment type
pub const EEPAR_ENVIRONMENT: u32 = 1;
/// Parameter set version, unique within environment type
pub const EEPAR_VERSION: u32 = 2;
/// Logical stack limit, in stack items
pub const EEPAR_STACK_LOGICAL_MAX: u32 = 3;
/// Native stack limit, in bytes
pub const EEPAR_STACK_NATIVE_MAX: u32 = 4;

/// # Execution enviroment types
/// Generic Wasmtime environment
pub const EXEC_ENV_TYPE_WASMTIME_GENERIC: u32 = 0;

/// Deterministically serialized execution environment semantics
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, MallocSizeOf, TypeInfo)]
pub struct ExecutorParams(Vec<(u32, Vec<u8>)>);

impl ExecutorParams {
	/// Creates a new, empty executor parameter set
	pub fn new() -> Self {
		ExecutorParams(Vec::new())
	}

	/// Returns globally unique version of execution environment parameter set, if present. Otherwise, returns 0.
	pub fn version(&self) -> u64 {
		if self.0.len() < 2 || self.0[0].0 != EEPAR_ENVIRONMENT || self.0[1].0 != EEPAR_VERSION {
			return 0
		}
		let env_enc = self.0[0].1.clone();
		let ver_enc = self.0[1].1.clone();
		match (u32::decode(&mut &env_enc[..]), u32::decode(&mut &ver_enc[..])) {
			(Ok(env), Ok(ver)) => (env as u64) * 2 ^ 32u64 + ver as u64,
			_ => 0,
		}
	}

	/// Returns execution environment type identifier
	pub fn environment(&self) -> u32 {
		if self.0.len() < 1 || self.0[0].0 != EEPAR_ENVIRONMENT {
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
		// TODO: Check for determinictic order of tags
		self.0.push((tag, value.encode()));
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

// impl From<&[(u32, &[u8])]> for ExecutorParams {
// 	fn from(arr: &[(u32, &[u8])]) -> Self {
// 		ExecutorParams(arr.into_iter().map(|e| (e.0, e.1.to_vec())).collect())
// 	}
// }

// impl From<&[(u32, &dyn Encode)]> for ExecutorParams {
// 	fn from(arr: &[(u32, impl Encode)]) -> Self {
// 		ExecutorParams(arr.into_iter().map(|e| (e.0, e.1.encode())).collect())
// 	}
// }
