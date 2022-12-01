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
use sp_std::{ops::Deref, vec, vec::Vec};

/// Execution environment type
#[cfg_attr(feature = "std", derive(MallocSizeOf))]
#[derive(Clone, Copy, Debug, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum ExecutionEnvironment {
	/// Generic Wasmtime executor
	WasmtimeGeneric = 0,
}

/// Executor instantiation strategy
#[cfg_attr(feature = "std", derive(MallocSizeOf))]
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum ExecInstantiationStrategy {
	/// Pooling copy-on-write
	PoolingCoW,
	/// Recreate instance copy-on-write
	RecreateCoW,
	/// Pooling
	Pooling,
	/// Recreate instance
	Recreate,
}

/// A single executor parameter
#[cfg_attr(feature = "std", derive(MallocSizeOf))]
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum ExecutorParam {
	/// ## General parameters:
	/// Execution environment type
	Environment(ExecutionEnvironment),

	/// ## Parameters setting the executuion environment semantics:
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

impl ExecutorParam {
	fn is_environment(&self) -> bool {
		match self {
			ExecutorParam::Environment(_) => true,
			_ => false,
		}
	}
}

/// Unit type wrapper around [`type@Hash`] that represents an execution parameter set hash.
///
/// This type is produced by [`ExecutorParams::hash`].
#[derive(Clone, Copy, Encode, Decode, Hash, Eq, PartialEq, PartialOrd, Ord, TypeInfo)]
pub struct ExecutorParamsHash(Hash);

impl ExecutorParamsHash {
	/// Create a new executor parameter hash from `H256` hash
	pub fn from_hash(hash: Hash) -> Self {
		Self(hash)
	}
}

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

/// # Deterministically serialized execution environment semantics
/// Represents an arbitrary semantics of an arbitrary execution environment, so should be kept as
/// abstract as possible. Mapping from `u32` constant tags to SCALE-encoded values was chosen
/// over an idiomatic `enum` representation as the latter would require caller to rely on
/// deserialization errors when decoding a structure of newer version not yet known to the node
/// software, which is undesirable.
/// As this one goes on-chain, it requires full serialization determinism. That is achieved by
/// enforcing constant tags to be in ascending order and using deterministic SCALE-codec for values.
#[cfg_attr(feature = "std", derive(MallocSizeOf))]
#[derive(Clone, Debug, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub struct ExecutorParams(Vec<ExecutorParam>);

impl ExecutorParams {
	/// Creates a new, empty executor parameter set
	pub fn new(environment: ExecutionEnvironment) -> Self {
		ExecutorParams(vec![ExecutorParam::Environment(environment)])
	}

	/// Returns execution environment type
	pub fn environment(&self) -> ExecutionEnvironment {
		debug_assert!(self.0.len() > 0);
		if let ExecutorParam::Environment(environment) = self.0[0] {
			environment
		} else {
			unreachable!();
		}
	}

	/// Returns hash of the set of execution environment parameters
	pub fn hash(&self) -> ExecutorParamsHash {
		ExecutorParamsHash(BlakeTwo256::hash(&self.encode()))
	}
}

impl Deref for ExecutorParams {
	type Target = Vec<ExecutorParam>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl From<&[ExecutorParam]> for ExecutorParams {
	fn from(arr: &[ExecutorParam]) -> Self {
		let vec = arr.to_vec();
		debug_assert!(vec.len() > 0);
		debug_assert!(vec[0].is_environment());
		ExecutorParams(vec)
	}
}

impl Default for ExecutorParams {
	fn default() -> Self {
		ExecutorParams(vec![ExecutorParam::Environment(ExecutionEnvironment::WasmtimeGeneric)])
	}
}
