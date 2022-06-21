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
use std::fmt;

#[derive(Debug, serde::Serialize)]
pub(crate) struct SimpleRuntimeVersion {
	pub spec_name: String,
	pub spec_version: u32,
}

impl fmt::Display for SimpleRuntimeVersion {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		writeln!(f, "{} v{}", self.spec_name, self.spec_version)
	}
}

impl From<&RuntimeVersion> for SimpleRuntimeVersion {
	fn from(r: &RuntimeVersion) -> Self {
		Self { spec_name: r.spec_name.to_string(), spec_version: r.spec_version }
	}
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct RuntimeVersions {
	pub builtin: SimpleRuntimeVersion,
	pub remote: SimpleRuntimeVersion,
}

impl RuntimeVersions {
	pub fn new(
		remote_runtime_version: &RuntimeVersion,
		builtin_runtime_version: &RuntimeVersion,
	) -> Self {
		Self { remote: remote_runtime_version.into(), builtin: builtin_runtime_version.into() }
	}
}

impl fmt::Display for RuntimeVersions {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let _ = write!(f, "- builtin: {}", self.builtin);
		write!(f, "- remote : {}", self.remote)
	}
}
