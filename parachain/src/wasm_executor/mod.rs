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

use std::any::{TypeId, Any};
use crate::{ValidationParams, ValidationResult, UpwardMessage, TargetedMessage};
use codec::{Decode, Encode};
use sp_core::storage::{ChildStorageKey, ChildInfo};

#[cfg(not(target_os = "unknown"))]
pub use validation_host::{run_worker, EXECUTION_TIMEOUT_SEC};

mod validation_host;

// maximum memory in bytes
const MAX_RUNTIME_MEM: usize = 1024 * 1024 * 1024; // 1 GiB
const MAX_CODE_MEM: usize = 16 * 1024 * 1024; // 16 MiB

sp_externalities::decl_extension! {
	/// The extension that is registered at the `Externalities` when validating a parachain state
	/// transition.
	pub(crate) struct ParachainExt(Box<dyn Externalities>);
}

impl ParachainExt {
	pub fn new<T: Externalities + 'static>(ext: T) -> Self {
		Self(Box::new(ext))
	}
}

/// WASM code execution mode.
///
/// > Note: When compiling for WASM, the `Remote` variants are not available.
pub enum ExecutionMode {
	/// Execute in-process. The execution can not be interrupted or aborted.
	Local,
	/// Remote execution in a spawned process.
	Remote,
	/// Remote execution in a spawned test runner.
	RemoteTest,
}

/// Error type for the wasm executor
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Wasm executor error.
	#[display(fmt = "WASM executor error: {:?}", _0)]
	WasmExecutor(sc_executor::error::Error),
	/// Call data is too large.
	#[display(fmt = "Validation parameters are {} bytes, max allowed is {}", _0, MAX_RUNTIME_MEM)]
	#[from(ignore)]
	ParamsTooLarge(usize),
	/// Code size it too large.
	#[display(fmt = "WASM code is {} bytes, max allowed is {}", _0, MAX_CODE_MEM)]
	CodeTooLarge(usize),
	/// Bad return data or type.
	#[display(fmt = "Validation function returned invalid data.")]
	BadReturn,
	#[display(fmt = "Validation function timeout.")]
	Timeout,
	#[display(fmt = "IO error: {}", _0)]
	Io(std::io::Error),
	#[display(fmt = "System error: {}", _0)]
	System(Box<dyn std::error::Error>),
	#[display(fmt = "WASM worker error: {}", _0)]
	External(String),
	#[display(fmt = "Shared memory error: {}", _0)]
	#[cfg(not(target_os = "unknown"))]
	SharedMem(shared_memory::SharedMemError),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::WasmExecutor(ref err) => Some(err),
			Error::Io(ref err) => Some(err),
			Error::System(ref err) => Some(&**err),
			#[cfg(not(target_os = "unknown"))]
			Error::SharedMem(ref err) => Some(err),
			_ => None,
		}
	}
}

/// Externalities for parachain validation.
pub trait Externalities: Send {
	/// Called when a message is to be posted to another parachain.
	fn post_message(&mut self, message: TargetedMessage) -> Result<(), String>;

	/// Called when a message is to be posted to the parachain's relay chain.
	fn post_upward_message(&mut self, message: UpwardMessage) -> Result<(), String>;
}

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate<E: Externalities + 'static>(
	validation_code: &[u8],
	params: ValidationParams,
	ext: E,
	options: ExecutionMode,
) -> Result<ValidationResult, Error> {
	match options {
		ExecutionMode::Local => {
			validate_candidate_internal(validation_code, &params.encode(), ext)
		},
		#[cfg(not(target_os = "unknown"))]
		ExecutionMode::Remote => {
			validation_host::validate_candidate(validation_code, params, ext, false)
		},
		#[cfg(not(target_os = "unknown"))]
		ExecutionMode::RemoteTest => {
			validation_host::validate_candidate(validation_code, params, ext, true)
		},
		#[cfg(target_os = "unknown")]
		ExecutionMode::Remote =>
			Err(Error::System("Remote validator not available".to_string().into())),
		#[cfg(target_os = "unknown")]
		ExecutionMode::RemoteTest =>
			Err(Error::System("Remote validator not available".to_string().into())),
	}
}

/// The host functions provided by the wasm executor to the parachain wasm blob.
type HostFunctions = (sp_io::SubstrateHostFunctions, crate::wasm_api::parachain::HostFunctions);

/// Validate a candidate under the given validation code.
///
/// This will fail if the validation code is not a proper parachain validation module.
pub fn validate_candidate_internal<E: Externalities + 'static>(
	validation_code: &[u8],
	encoded_call_data: &[u8],
	externalities: E,
) -> Result<ValidationResult, Error> {
	let mut ext = ValidationExternalities(ParachainExt::new(externalities));

	let res = sc_executor::call_in_wasm::<_, HostFunctions>(
		"validate_block",
		encoded_call_data,
		sc_executor::WasmExecutionMethod::Interpreted,
		&mut ext,
		validation_code,
		// TODO: Make sure we don't use more than 1GB: https://github.com/paritytech/polkadot/issues/699
		1024,
	)?;

	ValidationResult::decode(&mut &res[..]).map_err(|_| Error::BadReturn.into())
}

/// The validation externalities that will panic on any storage related access. They just provide
/// access to the parachain extension.
struct ValidationExternalities(ParachainExt);

impl sp_externalities::Externalities for ValidationExternalities {
	fn storage(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("storage: unsupported feature for parachain validation")
	}

	fn storage_hash(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("storage_hash: unsupported feature for parachain validation")
	}

	fn child_storage_hash(&self, _: ChildStorageKey, _: ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("child_storage_hash: unsupported feature for parachain validation")
	}

	fn original_storage(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("original_sorage: unsupported feature for parachain validation")
	}

	fn original_child_storage(&self, _: ChildStorageKey, _: ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("original_child_storage: unsupported feature for parachain validation")
	}

	fn original_storage_hash(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("original_storage_hash: unsupported feature for parachain validation")
	}

	fn original_child_storage_hash(&self, _: ChildStorageKey, _: ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("original_child_storage_hash: unsupported feature for parachain validation")
	}

	fn child_storage(&self, _: ChildStorageKey, _: ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("child_storage: unsupported feature for parachain validation")
	}

	fn kill_child_storage(&mut self, _: ChildStorageKey, _: ChildInfo) {
		panic!("kill_child_storage: unsupported feature for parachain validation")
	}

	fn clear_prefix(&mut self, _: &[u8]) {
		panic!("clear_prefix: unsupported feature for parachain validation")
	}

	fn clear_child_prefix(&mut self, _: ChildStorageKey, _: ChildInfo, _: &[u8]) {
		panic!("clear_child_prefix: unsupported feature for parachain validation")
	}

	fn place_storage(&mut self, _: Vec<u8>, _: Option<Vec<u8>>) {
		panic!("place_storage: unsupported feature for parachain validation")
	}

	fn place_child_storage(&mut self, _: ChildStorageKey, _: ChildInfo, _: Vec<u8>, _: Option<Vec<u8>>) {
		panic!("place_child_storage: unsupported feature for parachain validation")
	}

	fn chain_id(&self) -> u64 {
		panic!("chain_id: unsupported feature for parachain validation")
	}

	fn storage_root(&mut self) -> Vec<u8> {
		panic!("storage_root: unsupported feature for parachain validation")
	}

	fn child_storage_root(&mut self, _: ChildStorageKey) -> Vec<u8> {
		panic!("child_storage_root: unsupported feature for parachain validation")
	}

	fn storage_changes_root(&mut self, _: &[u8]) -> Result<Option<Vec<u8>>, ()> {
		panic!("storage_changes_root: unsupported feature for parachain validation")
	}

	fn next_child_storage_key(&self, _: ChildStorageKey, _: ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("next_child_storage_key: unsupported feature for parachain validation")
	}

	fn next_storage_key(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("next_storage_key: unsupported feature for parachain validation")
	}
}

impl sp_externalities::ExtensionStore for ValidationExternalities {
	fn extension_by_type_id(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
		if type_id == TypeId::of::<ParachainExt>() {
			Some(&mut self.0)
		} else {
			None
		}
	}
}
