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

fn main() {
	let builder = polkadot_node_core_pvf_musl_builder::Builder::new()
		// Tell the builder to build the project (crate) this `build.rs` is part of.
		.with_current_project();

	// Only require musl on supported secure-mode platforms.
	#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
	let builder = builder.with_target("x86_64-unknown-linux-musl");
	#[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
	let builder = builder.with_current_target();

	builder
		.set_file_name("execute-worker.rs")
		.set_constant_name("EXECUTE_EXE")
		// Build it.
		.build();
}
