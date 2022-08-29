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

use sc_executor_common::{
	runtime_blob::RuntimeBlob,
	wasm_runtime::{InvokeMethod, WasmModule as _},
};
use sc_executor_wasmtime::{Config, DeterministicStackLimit, Semantics};
use sp_core::storage::{ChildInfo, TrackedStorageKey};
use sp_externalities::MultiRemovalResults;
use std::{
	any::{Any, TypeId},
	path::Path,
};

// Memory configuration
//
// When Substrate Runtime is instantiated, a number of WASM pages are allocated for the Substrate
// Runtime instance's linear memory. The exact number of pages is a sum of whatever the WASM blob
// itself requests (by default at least enough to hold the data section as well as have some space
// left for the stack; this is, of course, overridable at link time when compiling the runtime)
// plus the number of pages specified in the `extra_heap_pages` passed to the executor.
//
// By default, rustc (or `lld` specifically) should allocate 1 MiB for the shadow stack, or 16 pages.
// The data section for runtimes are typically rather small and can fit in a single digit number of
// WASM pages, so let's say an extra 16 pages. Thus let's assume that 32 pages or 2 MiB are used for
// these needs by default.
const DEFAULT_HEAP_PAGES_ESTIMATE: u64 = 32;
const EXTRA_HEAP_PAGES: u64 = 2048;

/// The number of bytes devoted for the stack during wasm execution of a PVF.
const NATIVE_STACK_MAX: u32 = 256 * 1024 * 1024;

const CONFIG: Config = Config {
	allow_missing_func_imports: true,
	cache_path: None,
	semantics: Semantics {
		extra_heap_pages: EXTRA_HEAP_PAGES,

		// NOTE: This is specified in bytes, so we multiply by WASM page size.
		max_memory_size: Some(((DEFAULT_HEAP_PAGES_ESTIMATE + EXTRA_HEAP_PAGES) * 65536) as usize),

		instantiation_strategy:
			sc_executor_wasmtime::InstantiationStrategy::RecreateInstanceCopyOnWrite,

		// Enable deterministic stack limit to pin down the exact number of items the wasmtime stack
		// can contain before it traps with stack overflow.
		//
		// Here is how the values below were chosen.
		//
		// At the moment of writing, the default native stack size limit is 1 MiB. Assuming a logical item
		// (see the docs about the field and the instrumentation algorithm) is 8 bytes, 1 MiB can
		// fit 2x 65536 logical items.
		//
		// Since reaching the native stack limit is undesirable, we halve the logical item limit and
		// also increase the native 256x. This hopefully should preclude wasm code from reaching
		// the stack limit set by the wasmtime.
		deterministic_stack_limit: Some(DeterministicStackLimit {
			logical_max: 65536,
			native_stack_max: NATIVE_STACK_MAX,
		}),
		canonicalize_nans: true,
		// Rationale for turning the multi-threaded compilation off is to make the preparation time
		// easily reproducible and as deterministic as possible.
		//
		// Currently the prepare queue doesn't distinguish between precheck and prepare requests.
		// On the one hand, it simplifies the code, on the other, however, slows down compile times
		// for execute requests. This behavior may change in future.
		parallel_compilation: false,
	},
};

/// Runs the prevalidation on the given code. Returns a [`RuntimeBlob`] if it succeeds.
pub fn prevalidate(code: &[u8]) -> Result<RuntimeBlob, sc_executor_common::error::WasmError> {
	let blob = RuntimeBlob::new(code)?;
	// It's assumed this function will take care of any prevalidation logic
	// that needs to be done.
	//
	// Do nothing for now.
	Ok(blob)
}

/// Runs preparation on the given runtime blob. If successful, it returns a serialized compiled
/// artifact which can then be used to pass into [`execute`] after writing it to the disk.
pub fn prepare(blob: RuntimeBlob) -> Result<Vec<u8>, sc_executor_common::error::WasmError> {
	sc_executor_wasmtime::prepare_runtime_artifact(blob, &CONFIG.semantics)
}

pub struct Executor {
	thread_pool: rayon::ThreadPool,
	spawner: TaskSpawner,
}

impl Executor {
	pub fn new() -> Result<Self, String> {
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
		//    and that will abort the process as well.
		//
		// Typically on Linux the main thread gets the stack size specified by the `ulimit` and
		// typically it's configured to 8 MiB. Rust's spawned threads are 2 MiB. OTOH, the
		// NATIVE_STACK_MAX is set to 256 MiB. Not nearly enough.
		//
		// Hence we need to increase it.
		//
		// The simplest way to fix that is to spawn a thread with the desired stack limit. In order
		// to avoid costs of creating a thread, we use a thread pool. The execution is
		// single-threaded hence the thread pool has only one thread.
		//
		// The reasoning why we pick this particular size is:
		//
		// The default Rust thread stack limit 2 MiB + 256 MiB wasm stack.
		let thread_stack_size = 2 * 1024 * 1024 + NATIVE_STACK_MAX as usize;
		let thread_pool = rayon::ThreadPoolBuilder::new()
			.num_threads(1)
			.stack_size(thread_stack_size)
			.build()
			.map_err(|e| format!("Failed to create thread pool: {:?}", e))?;

		let spawner =
			TaskSpawner::new().map_err(|e| format!("cannot create task spawner: {}", e))?;

		Ok(Self { thread_pool, spawner })
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
		compiled_artifact_path: &Path,
		params: &[u8],
	) -> Result<Vec<u8>, String> {
		let spawner = self.spawner.clone();
		let mut result = None;
		self.thread_pool.scope({
			let result = &mut result;
			move |s| {
				s.spawn(move |_| {
					// spawn does not return a value, so we need to use a variable to pass the result.
					*result = Some(
						do_execute(compiled_artifact_path, params, spawner)
							.map_err(|err| format!("execute error: {:?}", err)),
					);
				});
			}
		});
		result.unwrap_or_else(|| Err("rayon thread pool spawn failed".to_string()))
	}
}

unsafe fn do_execute(
	compiled_artifact_path: &Path,
	params: &[u8],
	spawner: impl sp_core::traits::SpawnNamed + 'static,
) -> Result<Vec<u8>, sc_executor_common::error::Error> {
	let mut extensions = sp_externalities::Extensions::new();

	extensions.register(sp_core::traits::TaskExecutorExt::new(spawner));
	extensions.register(sp_core::traits::ReadRuntimeVersionExt::new(ReadRuntimeVersion));

	let mut ext = ValidationExternalities(extensions);

	sc_executor::with_externalities_safe(&mut ext, || {
		let runtime = sc_executor_wasmtime::create_runtime_from_artifact::<HostFunctions>(
			compiled_artifact_path,
			CONFIG,
		)?;
		runtime.new_instance()?.call(InvokeMethod::Export("validate_block"), params)
	})?
}

type HostFunctions = (
	sp_io::misc::HostFunctions,
	sp_io::crypto::HostFunctions,
	sp_io::hashing::HostFunctions,
	sp_io::allocator::HostFunctions,
	sp_io::logging::HostFunctions,
	sp_io::trie::HostFunctions,
	sp_io::sandbox::HostFunctions,
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

/// An implementation of `SpawnNamed` on top of a futures' thread pool.
///
/// This is a light handle meaning it will only clone the handle not create a new thread pool.
#[derive(Clone)]
pub(crate) struct TaskSpawner(futures::executor::ThreadPool);

impl TaskSpawner {
	pub(crate) fn new() -> Result<Self, String> {
		futures::executor::ThreadPoolBuilder::new()
			.pool_size(4)
			.name_prefix("pvf-task-executor")
			.create()
			.map_err(|e| e.to_string())
			.map(Self)
	}
}

impl sp_core::traits::SpawnNamed for TaskSpawner {
	fn spawn_blocking(
		&self,
		_task_name: &'static str,
		_subsystem_name: Option<&'static str>,
		future: futures::future::BoxFuture<'static, ()>,
	) {
		self.0.spawn_ok(future);
	}

	fn spawn(
		&self,
		_task_name: &'static str,
		_subsystem_name: Option<&'static str>,
		future: futures::future::BoxFuture<'static, ()>,
	) {
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
