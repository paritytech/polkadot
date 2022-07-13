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
pub(crate) struct RuntimeWrapper<'a>(pub &'a RuntimeVersion);

impl<'a> fmt::Display for RuntimeWrapper<'a> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let width = 16;

		writeln!(
			f,
			r#"   impl_name           : {impl_name:>width$}
   spec_name           : {spec_name:>width$}
   spec_version        : {spec_version:>width$}
   transaction_version : {transaction_version:>width$}
   impl_version        : {impl_version:>width$}
   authoringVersion    : {authoring_version:>width$}
   state_version       : {state_version:>width$}"#,
			spec_name = self.0.spec_name.to_string(),
			impl_name = self.0.impl_name.to_string(),
			spec_version = self.0.spec_version,
			impl_version = self.0.impl_version,
			authoring_version = self.0.authoring_version,
			transaction_version = self.0.transaction_version,
			state_version = self.0.state_version,
		)
	}
}

impl<'a> From<&'a RuntimeVersion> for RuntimeWrapper<'a> {
	fn from(r: &'a RuntimeVersion) -> Self {
		RuntimeWrapper(r)
	}
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct RuntimeVersions<'a> {
	/// The `RuntimeVersion` linked in the staking-miner
	pub linked: RuntimeWrapper<'a>,

	/// The `RuntimeVersion` reported by the node we connect to via RPC
	pub remote: RuntimeWrapper<'a>,

	/// This `bool` reports whether both remote and linked `RuntimeVersion` are compatible
	/// and if the staking-miner is expected to work properly against the remote runtime
	compatible: bool,
}

impl<'a> RuntimeVersions<'a> {
	pub fn new(
		remote_runtime_version: &'a RuntimeVersion,
		linked_runtime_version: &'a RuntimeVersion,
	) -> Self {
		Self {
			remote: remote_runtime_version.into(),
			linked: linked_runtime_version.into(),
			compatible: are_runtimes_compatible(remote_runtime_version, linked_runtime_version),
		}
	}
}

/// Check whether runtimes are compatible. Currently we only support equality.
fn are_runtimes_compatible(r1: &RuntimeVersion, r2: &RuntimeVersion) -> bool {
	r1 == r2
}

impl<'a> fmt::Display for RuntimeVersions<'a> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let _ = write!(f, "- linked:\n{}", self.linked);
		let _ = write!(f, "- remote :\n{}", self.remote);
		write!(f, "Compatible: {}", if self.compatible { "YES" } else { "NO" })
	}
}
