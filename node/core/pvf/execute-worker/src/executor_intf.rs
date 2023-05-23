// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Interface to the Substrate Executor

use polkadot_node_core_pvf_common::executor_intf::{
	params_to_wasmtime_semantics, DEFAULT_CONFIG, NATIVE_STACK_MAX,
};
use polkadot_primitives::ExecutorParams;
use sc_executor_common::{
	error::WasmError,
	runtime_blob::RuntimeBlob,
	wasm_runtime::{InvokeMethod, WasmModule as _},
};
use sc_executor_wasmtime::{Config, WasmtimeRuntime};
use sp_core::storage::{ChildInfo, TrackedStorageKey};
use sp_externalities::MultiRemovalResults;
use std::any::{Any, TypeId};

// Wasmtime powers the Substrate Executor. It compiles the wasm bytecode into native code.
// That native code does not create any stacks and just reuses the stack of the thread that
// wasmtime was invoked from.
//
// Also, we configure the executor to provide the deterministic stack and that requires
// supplying the amount of the native stack space that wasm is allowed to use. This is
// realized by supplying the limit into `wasmtime::Config::max_wasm_stack`.
//
// There are quirks to that configuration knob:
//
// 1. It only limits the amount of stack space consumed by wasm but does not ensure nor check
//    that the stack space is actually available.
//
//    That means, if the calling thread has 1 MiB of stack space left and the wasm code consumes
//    more, then the wasmtime limit will **not** trigger. Instead, the wasm code will hit the
//    guard page and the Rust stack overflow handler will be triggered. That leads to an
//    **abort**.
//
// 2. It cannot and does not limit the stack space consumed by Rust code.
//
//    Meaning that if the wasm code leaves no stack space for Rust code, then the Rust code
//    will abort and that will abort the process as well.
//
// Typically on Linux the main thread gets the stack size specified by the `ulimit` and
// typically it's configured to 8 MiB. Rust's spawned threads are 2 MiB. OTOH, the
// NATIVE_STACK_MAX is set to 256 MiB. Not nearly enough.
//
// Hence we need to increase it. The simplest way to fix that is to spawn a thread with the desired
// stack limit.
//
// The reasoning why we pick this particular size is:
//
// The default Rust thread stack limit 2 MiB + 256 MiB wasm stack.
/// The stack size for the execute thread.
pub const EXECUTE_THREAD_STACK_SIZE: usize = 2 * 1024 * 1024 + NATIVE_STACK_MAX as usize;

#[derive(Clone)]
pub struct Executor {
	config: Config,
}

impl Executor {
	pub fn new(params: ExecutorParams) -> Result<Self, String> {
		let mut config = DEFAULT_CONFIG.clone();
		config.semantics = params_to_wasmtime_semantics(&params)?;

		Ok(Self { config })
	}

	/// Executes the given PVF in the form of a compiled artifact and returns the result of execution
	/// upon success.
	///
	/// # Safety
	///
	/// The caller must ensure that the compiled artifact passed here was:
	///   1) produced by [`prepare`],
	///   2) written to the disk as a file,
	///   3) was not modified,
	///   4) will not be modified while any runtime using this artifact is alive, or is being
	///      instantiated.
	///
	/// Failure to adhere to these requirements might lead to crashes and arbitrary code execution.
	pub unsafe fn execute(
		&self,
		compiled_artifact_blob: &[u8],
		params: &[u8],
	) -> Result<Vec<u8>, String> {
		let mut extensions = sp_externalities::Extensions::new();

		extensions.register(sp_core::traits::ReadRuntimeVersionExt::new(ReadRuntimeVersion));

		let mut ext = ValidationExternalities(extensions);

		match sc_executor::with_externalities_safe(&mut ext, || {
			let runtime = self.create_runtime_from_bytes(compiled_artifact_blob)?;
			runtime.new_instance()?.call(InvokeMethod::Export("validate_block"), params)
		}) {
			Ok(Ok(ok)) => Ok(ok),
			Ok(Err(err)) | Err(err) => Err(err),
		}
		.map_err(|err| format!("execute error: {:?}", err))
	}

	/// Constructs the runtime for the given PVF, given the artifact bytes.
	///
	/// # Safety
	///
	/// The caller must ensure that the compiled artifact passed here was:
	///   1) produced by [`prepare`],
	///   2) was not modified,
	///
	/// Failure to adhere to these requirements might lead to crashes and arbitrary code execution.
	pub unsafe fn create_runtime_from_bytes(
		&self,
		compiled_artifact_blob: &[u8],
	) -> Result<WasmtimeRuntime, WasmError> {
		sc_executor_wasmtime::create_runtime_from_artifact_bytes::<HostFunctions>(
			compiled_artifact_blob,
			self.config.clone(),
		)
	}
}

type HostFunctions = (
	sp_io::misc::HostFunctions,
	sp_io::crypto::HostFunctions,
	sp_io::hashing::HostFunctions,
	sp_io::allocator::HostFunctions,
	sp_io::logging::HostFunctions,
	sp_io::trie::HostFunctions,
);

/// The validation externalities that will panic on any storage related access.
struct ValidationExternalities(sp_externalities::Extensions);

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

	fn kill_child_storage(
		&mut self,
		_child_info: &ChildInfo,
		_maybe_limit: Option<u32>,
		_maybe_cursor: Option<&[u8]>,
	) -> MultiRemovalResults {
		panic!("kill_child_storage: unsupported feature for parachain validation")
	}

	fn clear_prefix(
		&mut self,
		_prefix: &[u8],
		_maybe_limit: Option<u32>,
		_maybe_cursor: Option<&[u8]>,
	) -> MultiRemovalResults {
		panic!("clear_prefix: unsupported feature for parachain validation")
	}

	fn clear_child_prefix(
		&mut self,
		_child_info: &ChildInfo,
		_prefix: &[u8],
		_maybe_limit: Option<u32>,
		_maybe_cursor: Option<&[u8]>,
	) -> MultiRemovalResults {
		panic!("clear_child_prefix: unsupported feature for parachain validation")
	}

	fn place_storage(&mut self, _: Vec<u8>, _: Option<Vec<u8>>) {
		panic!("place_storage: unsupported feature for parachain validation")
	}

	fn place_child_storage(&mut self, _: &ChildInfo, _: Vec<u8>, _: Option<Vec<u8>>) {
		panic!("place_child_storage: unsupported feature for parachain validation")
	}

	fn storage_root(&mut self, _: sp_core::storage::StateVersion) -> Vec<u8> {
		panic!("storage_root: unsupported feature for parachain validation")
	}

	fn child_storage_root(&mut self, _: &ChildInfo, _: sp_core::storage::StateVersion) -> Vec<u8> {
		panic!("child_storage_root: unsupported feature for parachain validation")
	}

	fn next_child_storage_key(&self, _: &ChildInfo, _: &[u8]) -> Option<Vec<u8>> {
		panic!("next_child_storage_key: unsupported feature for parachain validation")
	}

	fn next_storage_key(&self, _: &[u8]) -> Option<Vec<u8>> {
		panic!("next_storage_key: unsupported feature for parachain validation")
	}

	fn storage_append(&mut self, _key: Vec<u8>, _value: Vec<u8>) {
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

	fn get_read_and_written_keys(&self) -> Vec<(Vec<u8>, u32, u32, bool)> {
		panic!("get_read_and_written_keys: unsupported feature for parachain validation")
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

struct ReadRuntimeVersion;

impl sp_core::traits::ReadRuntimeVersion for ReadRuntimeVersion {
	fn read_runtime_version(
		&self,
		wasm_code: &[u8],
		_ext: &mut dyn sp_externalities::Externalities,
	) -> Result<Vec<u8>, String> {
		let blob = RuntimeBlob::uncompress_if_needed(wasm_code)
			.map_err(|e| format!("Failed to read the PVF runtime blob: {:?}", e))?;

		match sc_executor::read_embedded_version(&blob)
			.map_err(|e| format!("Failed to read the static section from the PVF blob: {:?}", e))?
		{
			Some(version) => {
				use parity_scale_codec::Encode;
				Ok(version.encode())
			},
			None => Err("runtime version section is not found".to_string()),
		}
	}
}
