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

/// Execution environment type
#[derive(Clone, Copy, Debug, Encode, Decode, PartialEq, Eq, MallocSizeOf, TypeInfo)]
pub enum ExecutionEnvironment {
	/// Generic Wasmtime executor
	WasmtimeGeneric = 0,
}

/// Executor instantiation strategy
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, MallocSizeOf, TypeInfo)]
pub enum ExecInstantiationStrategy {
	/// Pooling copy-on-write
	PoolingCoW,
	/// Recreate instance copy-on-write
	RecreateCoW,
	/// Pooling
	Pooling,
	/// Recreate instance
	Recreate,
	/// Legacy instantiation strategy
	Legacy,
}

/// A single executor parameter
#[non_exhaustive]
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, MallocSizeOf, TypeInfo)]
pub enum ExecutorParam {
	/// General parameters:
	/// Execution environment type and a version of the set of execution parameters, unique in
	/// the scope of execution environment type
	Version(ExecutionEnvironment, u32),

	/// Parameters setting the executuion environment semantics:
	/// Number of extra heap pages
	ExtraHeapPages(u64),
	/// Max. memory size
	MaxMemorySize(u32),
	/// Wasm logical stack size limit, in units
	StackLogicalMax(u32),
	/// Executor machine stack size limit, in bytes
	StackNativeMax(u32),
	/// Executor instantiation strategy
	InstantiationStrategy(ExecInstantiationStrategy),
	/// `true` if compiler should perform NaNs canonicalization
	CanonicalizeNaNs(bool),
	/// `true` if parallel compilation is allowed, single thread is used otherwise
	ParallelCompilation(bool),
}

/// Deterministically serialized execution environment semantics
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, MallocSizeOf, TypeInfo)]
pub struct ExecutorParams(Vec<ExecutorParam>);

impl ExecutorParams {
	/// Returns globally unique version of execution environment parameter set, if present. Otherwise, returns 0.
	pub fn version(&self) -> u64 {
		if self.0.len() < 2 {
			return 0
		}
		let (env, ver) =
			if let ExecutorParam::Version(env, ver) = self.0[0] { (env, ver) } else { return 0 };
		(env as u64) * 2 ^ 32u64 + ver as u64
	}

	/// Returns execution environment type identifier
	pub fn environment(&self) -> ExecutionEnvironment {
		if self.0.len() < 1 {
			return ExecutionEnvironment::WasmtimeGeneric
		}
		if let ExecutorParam::Version(env, _) = self.0[0] {
			env
		} else {
			ExecutionEnvironment::WasmtimeGeneric
		}
	}
}

impl Deref for ExecutorParams {
	type Target = Vec<ExecutorParam>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl Default for ExecutorParams {
	fn default() -> Self {
		ExecutorParams(Vec::new())
	}
}

impl From<&[ExecutorParam]> for ExecutorParams {
	fn from(arr: &[ExecutorParam]) -> Self {
		ExecutorParams(arr.to_vec())
	}
}
