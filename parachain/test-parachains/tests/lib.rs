// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

mod adder;
mod wasm_executor;

use parachain::wasm_executor::run_worker;

// This is not an actual test, but rather an entry point for out-of process WASM executor.
// When executing tests the executor spawns currently executing binary, which happens to be test binary.
// It then passes "validation_worker" on CLI effectivly making rust test executor to run this single test.
#[test]
fn validation_worker() {
	if let Some(id) = std::env::args().find(|a| a.starts_with("/shmem_")) {
		run_worker(&id, None).unwrap()
	}
}
