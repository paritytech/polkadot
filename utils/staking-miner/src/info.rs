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

use sp_version::RuntimeVersion;

#[derive(Debug, serde::Serialize)]
pub(crate) struct Info {
	pub spec_name: String,
	pub spec_version: u32,
}

impl Info {
	pub fn new(runtime_version: &RuntimeVersion) -> Self {
		let spec_name = runtime_version.spec_name.to_string();
		let spec_version = runtime_version.spec_version;
		Self { spec_name, spec_version }
	}
}
