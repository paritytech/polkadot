// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! WASM re-execution of a parachain candidate.
//! In the context of relay-chain candidate evaluation, there are some additional
//! steps to ensure that the provided input parameters are correct.
//! Assuming the parameters are correct, this module provides a wrapper around
//! a WASM VM for re-execution of a parachain candidate.

use std::{any::{TypeId, Any}, path::PathBuf};
use crate::primitives::{ValidationParams, ValidationResult};
use codec::{Decode, Encode};
use sp_core::{storage::{ChildInfo, TrackedStorageKey}, traits::{CallInWasm, SpawnNamed}};
use sp_externalities::Extensions;
use sp_wasm_interface::HostFunctions as _;

#[cfg(not(any(target_os = "android", target_os = "unknown")))]
pub use validation_host::{run_worker, ValidationPool, EXECUTION_TIMEOUT_SEC, WORKER_ARGS};

mod validation_host;

// maximum memory in bytes
const MAX_RUNTIME_MEM: usize = 1024 * 1024 * 1024; // 1 GiB
const MAX_CODE_MEM: usize = 16 * 1024 * 1024; // 16 MiB
const MAX_VALIDATION_RESULT_HEADER_MEM: usize = MAX_CODE_MEM + 1024; // 16.001 MiB

/// A stub validation-pool defined when compiling for Android or WASM.
#[cfg(any(target_os = "android", target_os = "unknown"))]
#[derive(Clone)]
pub struct ValidationPool {
	_inner: (), // private field means not publicly-instantiable
}

#[cfg(any(target_os = "android", target_os = "unknown"))]
impl ValidationPool {
	/// Create a new `ValidationPool`.
	pub fn new() -> Self {
		ValidationPool { _inner: () }
	}
}

/// A stub function defined when compiling for Android or WASM.
#[cfg(any(target_os = "android", target_os = "unknown"))]
pub fn run_worker(_: &str) -> Result<(), String> {
	Err("Cannot run validation worker on this platform".to_string())
}

/// The execution mode for the `ValidationPool`.
#[derive(Clone)]
#[cfg_attr(not(any(target_os = "android", target_os = "unknown")), derive(Debug))]
pub enum ExecutionMode {
	/// The validation worker is ran in a thread inside the same process.
	InProcess,
	/// The validation worker is ran using the process' executable and the subcommand `validation-worker` is passed
	/// following by the address of the shared memory.
	ExternalProcessSelfHost(ValidationPool),
	/// The validation worker is ran using the command provided and the argument provided. The address of the shared
	/// memory is added at the end of the arguments.
	ExternalProcessCustomHost {
		/// Validation pool.
		pool: ValidationPool,
		/// Path to the validation worker. The file must exists and be executable.
		binary: PathBuf,
		/// List of arguments passed to the validation worker. The address of the shared memory will be automatically
		/// added after the arguments.
		args: Vec<String>,
	},
}


#[derive(Debug, thiserror::Error)]
/// Candidate validation error.
pub enum ValidationError {
	/// Validation failed due to internal reasons. The candidate might still be valid.
	#[error(transparent)]
	Internal(#[from] InternalError),
	/// Candidate is invalid.
	#[error(transparent)]
	InvalidCandidate(#[from] InvalidCandidate),
}

/// Error type that indicates invalid candidate.
#[derive(Debug, thiserror::Error)]
pub enum InvalidCandidate {
	/// Wasm executor error.
	#[error("WASM executor error")]
	WasmExecutor(#[from] sc_executor::error::Error),
	/// Call data is too large.
	#[error("Validation parameters are {0} bytes, max allowed is {}", MAX_RUNTIME_MEM)]
	ParamsTooLarge(usize),
	/// Code size it too large.
	#[error("WASM code is {0} bytes, max allowed is {}", MAX_CODE_MEM)]
	CodeTooLarge(usize),
	/// Error decoding returned data.
	#[error("Validation function returned invalid data.")]
	BadReturn,
	#[error("Validation function timeout.")]
	Timeout,
	#[error("External WASM execution error: {0}")]
	ExternalWasmExecutor(String),
}

impl core::convert::From<String> for InvalidCandidate {
	fn from(s: String) -> Self {
		Self::ExternalWasmExecutor(s)
	}
}

/// Host error during candidate validation. This does not indicate an invalid candidate.
#[derive(Debug, thiserror::Error)]
pub enum InternalError {
	#[error("IO error: {0}")]
	Io(#[from] std::io::Error),

	#[error("System error: {0}")]
	System(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),

	#[cfg(not(any(target_os = "android", target_os = "unknown")))]
	#[error("Shared memory error: {0}")]
	SharedMem(#[from] shared_memory::SharedMemError),

	#[error("WASM worker error: {0}")]
	WasmWorker(String),
}


/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate(
	validation_code: &[u8],
	params: ValidationParams,
	execution_mode: &ExecutionMode,
	spawner: impl SpawnNamed + 'static,
) -> Result<ValidationResult, ValidationError> {
	match execution_mode {
		ExecutionMode::InProcess => {
			validate_candidate_internal(validation_code, &params.encode(), spawner)
		},
		#[cfg(not(any(target_os = "android", target_os = "unknown")))]
		ExecutionMode::ExternalProcessSelfHost(pool) => {
			pool.validate_candidate(validation_code, params)
		},
		#[cfg(not(any(target_os = "android", target_os = "unknown")))]
		ExecutionMode::ExternalProcessCustomHost { pool, binary, args } => {
			let args: Vec<&str> = args.iter().map(|x| x.as_str()).collect();
			pool.validate_candidate_custom(validation_code, params, binary, &args)
		},
		#[cfg(any(target_os = "android", target_os = "unknown"))]
		ExecutionMode::ExternalProcessSelfHost(_) | ExecutionMode::ExternalProcessCustomHost { .. } =>
			Err(ValidationError::Internal(InternalError::System(
				Box::<dyn std::error::Error + Send + Sync>::from(
					"Remote validator not available".to_string()
				) as Box<_>
			))),
	}
}

/// The host functions provided by the wasm executor to the parachain wasm blob.
type HostFunctions = sp_io::SubstrateHostFunctions;

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate_internal(
	validation_code: &[u8],
	encoded_call_data: &[u8],
	spawner: impl SpawnNamed + 'static,
) -> Result<ValidationResult, ValidationError> {
	let executor = sc_executor::WasmExecutor::new(
		sc_executor::WasmExecutionMethod::Interpreted,
		// TODO: Make sure we don't use more than 1GB: https://github.com/paritytech/polkadot/issues/699
		Some(1024),
		HostFunctions::host_functions(),
		8
	);

	let mut extensions = Extensions::new();
	extensions.register(sp_core::traits::TaskExecutorExt::new(spawner));
	extensions.register(sp_core::traits::CallInWasmExt::new(executor.clone()));

	let mut ext = ValidationExternalities(extensions);

	let res = executor.call_in_wasm(
		validation_code,
		None,
		"validate_block",
		encoded_call_data,
		&mut ext,
		sp_core::traits::MissingHostFunctions::Allow,
	).map_err(|e| ValidationError::InvalidCandidate(e.into()))?;

	ValidationResult::decode(&mut &res[..])
		.map_err(|_| ValidationError::InvalidCandidate(InvalidCandidate::BadReturn).into())
}

/// The validation externalities that will panic on any storage related access. They just provide
/// access to the parachain extension.
struct ValidationExternalities(Extensions);

impl sp_externalities::Externalities for ValidationExternalities {
	fn storage(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("storage: unsupported feature for parachain validation")
	}

	fn storage_hash(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("storage_hash: unsupported feature for parachain validation")
	}

	fn child_storage_hash(&self, _: &ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("child_storage_hash: unsupported feature for parachain validation")
	}

	fn child_storage(&self, _: &ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("child_storage: unsupported feature for parachain validation")
	}

	fn kill_child_storage(&mut self, _: &ChildInfo) {
		panic!("kill_child_storage: unsupported feature for parachain validation")
	}

	fn clear_prefix(&mut self, _: &[u8]) {
		panic!("clear_prefix: unsupported feature for parachain validation")
	}

	fn clear_child_prefix(&mut self, _: &ChildInfo, _: &[u8]) {
		panic!("clear_child_prefix: unsupported feature for parachain validation")
	}

	fn place_storage(&mut self, _: Vec<u8>, _: Option<Vec<u8>>) {
		panic!("place_storage: unsupported feature for parachain validation")
	}

	fn place_child_storage(&mut self, _: &ChildInfo, _: Vec<u8>, _: Option<Vec<u8>>) {
		panic!("place_child_storage: unsupported feature for parachain validation")
	}

	fn chain_id(&self) -> u64 {
		panic!("chain_id: unsupported feature for parachain validation")
	}

	fn storage_root(&mut self) -> Vec<u8> {
		panic!("storage_root: unsupported feature for parachain validation")
	}

	fn child_storage_root(&mut self, _: &ChildInfo) -> Vec<u8> {
		panic!("child_storage_root: unsupported feature for parachain validation")
	}

	fn storage_changes_root(&mut self, _: &[u8]) -> Result<Option<Vec<u8>>, ()> {
		panic!("storage_changes_root: unsupported feature for parachain validation")
	}

	fn next_child_storage_key(&self, _: &ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("next_child_storage_key: unsupported feature for parachain validation")
	}

	fn next_storage_key(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("next_storage_key: unsupported feature for parachain validation")
	}

	fn storage_append(
		&mut self,
		_key: Vec<u8>,
		_value: Vec<u8>,
	) {
		panic!("storage_append: unsupported feature for parachain validation")
	}

	fn storage_start_transaction(&mut self) {
		panic!("storage_start_transaction: unsupported feature for parachain validation")
	}

	fn storage_rollback_transaction(&mut self) -> Result<(), ()> {
		panic!("storage_rollback_transaction: unsupported feature for parachain validation")
	}

	fn storage_commit_transaction(&mut self) -> Result<(), ()> {
		panic!("storage_commit_transaction: unsupported feature for parachain validation")
	}

	fn wipe(&mut self) {
		panic!("wipe: unsupported feature for parachain validation")
	}

	fn commit(&mut self) {
		panic!("commit: unsupported feature for parachain validation")
	}

	fn read_write_count(&self) -> (u32, u32, u32, u32) {
		panic!("read_write_count: unsupported feature for parachain validation")
	}

	fn reset_read_write_count(&mut self) {
		panic!("reset_read_write_count: unsupported feature for parachain validation")
	}

	fn get_whitelist(&self) -> Vec<TrackedStorageKey> {
		panic!("get_whitelist: unsupported feature for parachain validation")
	}

	fn set_whitelist(&mut self, _: Vec<TrackedStorageKey>) {
		panic!("set_whitelist: unsupported feature for parachain validation")
	}

	fn set_offchain_storage(&mut self, _: &[u8], _: std::option::Option<&[u8]>) {
		panic!("set_offchain_storage: unsupported feature for parachain validation")
	}
}

impl sp_externalities::ExtensionStore for ValidationExternalities {
	fn extension_by_type_id(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
		self.0.get_mut(type_id)
	}

	fn register_extension_with_type_id(
		&mut self,
		type_id: TypeId,
		extension: Box<dyn sp_externalities::Extension>,
	) -> Result<(), sp_externalities::Error> {
		self.0.register_with_type_id(type_id, extension)
	}

	fn deregister_extension_by_type_id(
		&mut self,
		type_id: TypeId,
	) -> Result<(), sp_externalities::Error> {
		if self.0.deregister(type_id) {
			Ok(())
		} else {
			Err(sp_externalities::Error::ExtensionIsNotRegistered(type_id))
		}
	}
}
