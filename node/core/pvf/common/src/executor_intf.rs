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

use polkadot_primitives::{ExecutorParam, ExecutorParams};
use sc_executor_common::wasm_runtime::HeapAllocStrategy;
use sc_executor_wasmtime::{Config, DeterministicStackLimit, Semantics};

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
const DEFAULT_HEAP_PAGES_ESTIMATE: u32 = 32;
const EXTRA_HEAP_PAGES: u32 = 2048;

/// The number of bytes devoted for the stack during wasm execution of a PVF.
pub const NATIVE_STACK_MAX: u32 = 256 * 1024 * 1024;

// VALUES OF THE DEFAULT CONFIGURATION SHOULD NEVER BE CHANGED
// They are used as base values for the execution environment parametrization.
// To overwrite them, add new ones to `EXECUTOR_PARAMS` in the `session_info` pallet and perform
// a runtime upgrade to make them active.
pub const DEFAULT_CONFIG: Config = Config {
	allow_missing_func_imports: true,
	cache_path: None,
	semantics: Semantics {
		heap_alloc_strategy: sc_executor_common::wasm_runtime::HeapAllocStrategy::Dynamic {
			maximum_pages: Some(DEFAULT_HEAP_PAGES_ESTIMATE + EXTRA_HEAP_PAGES),
		},

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

		// WASM extensions. Only those that are meaningful to us may be controlled here. By default,
		// we're using WASM MVP, which means all the extensions are disabled. Nevertheless, some
		// extensions (e.g., sign extension ops) are enabled by Wasmtime and cannot be disabled.
		wasm_reference_types: false,
		wasm_simd: false,
		wasm_bulk_memory: false,
		wasm_multi_value: false,
	},
};

pub fn params_to_wasmtime_semantics(par: &ExecutorParams) -> Result<Semantics, String> {
	let mut sem = DEFAULT_CONFIG.semantics.clone();
	let mut stack_limit = if let Some(stack_limit) = sem.deterministic_stack_limit.clone() {
		stack_limit
	} else {
		return Err("No default stack limit set".to_owned())
	};

	for p in par.iter() {
		match p {
			ExecutorParam::MaxMemoryPages(max_pages) =>
				sem.heap_alloc_strategy =
					HeapAllocStrategy::Dynamic { maximum_pages: Some(*max_pages) },
			ExecutorParam::StackLogicalMax(slm) => stack_limit.logical_max = *slm,
			ExecutorParam::StackNativeMax(snm) => stack_limit.native_stack_max = *snm,
			ExecutorParam::WasmExtBulkMemory => sem.wasm_bulk_memory = true,
			// TODO: Not implemented yet; <https://github.com/paritytech/polkadot/issues/6472>.
			ExecutorParam::PrecheckingMaxMemory(_) => (),
			ExecutorParam::PvfPrepTimeout(_, _) | ExecutorParam::PvfExecTimeout(_, _) => (), // Not used here
		}
	}
	sem.deterministic_stack_limit = Some(stack_limit);
	Ok(sem)
}
