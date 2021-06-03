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

//! Interface to the Substrate Executor

use std::any::{TypeId, Any};
use sc_executor_common::{
	runtime_blob::RuntimeBlob,
	wasm_runtime::{InvokeMethod, WasmModule as _},
};
use sc_executor_wasmtime::{Config, Semantics};
use sp_core::{
	storage::{ChildInfo, TrackedStorageKey},
};
use sp_wasm_interface::HostFunctions as _;

const CONFIG: Config = Config {
	// TODO: Make sure we don't use more than 1GB: https://github.com/paritytech/polkadot/issues/699
	heap_pages: 1024,
	allow_missing_func_imports: true,
	cache_path: None,
	semantics: Semantics {
		fast_instance_reuse: false,
		stack_depth_metering: false,
	},
};

/// Runs the prevaldation on the given code. Returns a [`RuntimeBlob`] if it succeeds.
pub fn prevalidate(code: &[u8]) -> Result<RuntimeBlob, sc_executor_common::error::WasmError> {
	let blob = RuntimeBlob::new(code)?;
	// It's assumed this function will take care of any prevalidation logic
	// that needs to be done.
	//
	// Do nothing for now.
	Ok(blob)
}

/// Runs preparation on the given runtime blob. If successful, it returns a serialized compiled
/// artifact which can then be used to pass into [`execute`].
pub fn prepare(blob: RuntimeBlob) -> Result<Vec<u8>, sc_executor_common::error::WasmError> {
	sc_executor_wasmtime::prepare_runtime_artifact(blob, &CONFIG.semantics)
}

/// Executes the given PVF in the form of a compiled artifact and returns the result of execution
/// upon success.
///
/// # Safety
///
/// The compiled artifact must be produced with [`prepare`]. Not following this guidance can lead
/// to arbitrary code execution.
pub unsafe fn execute(
	compiled_artifact: &[u8],
	params: &[u8],
	spawner: impl sp_core::traits::SpawnNamed + 'static,
) -> Result<Vec<u8>, sc_executor_common::error::Error> {
	let mut extensions = sp_externalities::Extensions::new();

	extensions.register(sp_core::traits::TaskExecutorExt::new(spawner));
	extensions.register(sp_core::traits::ReadRuntimeVersionExt::new(ReadRuntimeVersion));

	let mut ext = ValidationExternalities(extensions);

	sc_executor::with_externalities_safe(&mut ext, || {
		let runtime = sc_executor_wasmtime::create_runtime_from_artifact(
			compiled_artifact,
			CONFIG,
			HostFunctions::host_functions(),
		)?;
		runtime
			.new_instance()?
			.call(InvokeMethod::Export("validate_block"), params)
	})?
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

	fn kill_child_storage(&mut self, _: &ChildInfo, _: Option<u32>) -> (bool, u32) {
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

/// An implementation of `SpawnNamed` on top of a futures' thread pool.
///
/// This is a light handle meaning it will only clone the handle not create a new thread pool.
#[derive(Clone)]
pub(crate) struct TaskExecutor(futures::executor::ThreadPool);

impl TaskExecutor {
	pub(crate) fn new() -> Result<Self, String> {
		futures::executor::ThreadPoolBuilder::new()
			.pool_size(4)
			.name_prefix("pvf-task-executor")
			.create()
			.map_err(|e| e.to_string())
			.map(Self)
	}
}

impl sp_core::traits::SpawnNamed for TaskExecutor {
	fn spawn_blocking(&self, _: &'static str, future: futures::future::BoxFuture<'static, ()>) {
		self.0.spawn_ok(future);
	}

	fn spawn(&self, _: &'static str, future: futures::future::BoxFuture<'static, ()>) {
		self.0.spawn_ok(future);
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
			None => Err(format!("runtime version section is not found")),
		}
	}
}
