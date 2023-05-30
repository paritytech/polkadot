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

use substrate_wasm_builder::WasmBuilder;

fn main() {
	WasmBuilder::new()
		.with_current_project()
		.export_heap_base()
		.disable_runtime_version_section_check()
		.build();

	enable_alloc_error_handler();
}

#[rustversion::before(1.68)]
fn enable_alloc_error_handler() {
	if !cfg!(feature = "std") {
		println!("cargo:rustc-cfg=enable_alloc_error_handler");
	}
}

#[rustversion::since(1.68)]
fn enable_alloc_error_handler() {}
