// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use std::env;
use substrate_wasm_builder::WasmBuilder;

// note: needs to be synced with rococo-runtime-constants::time hard-coded string literal in prod_or_fast macro.
const ROCOCO_EPOCH_DURATION_ENV: &str = "ROCOCO_EPOCH_DURATION";
const ROCOCO_FAST_RUNTIME_ENV: &str = "ROCOCO_FAST_RUNTIME";

fn main() {
	#[cfg(feature = "std")]
	{
		let mut builder =
			WasmBuilder::new().with_current_project().import_memory().export_heap_base();

		if env::var(ROCOCO_EPOCH_DURATION_ENV).is_ok() | env::var(ROCOCO_FAST_RUNTIME_ENV).is_ok() {
			builder = builder.enable_feature("fast-runtime")
		};

		builder.build();

		println!("cargo:rerun-if-env-changed={}", ROCOCO_EPOCH_DURATION_ENV);
		println!("cargo:rerun-if-env-changed={}", ROCOCO_FAST_RUNTIME_ENV);
	}
}
