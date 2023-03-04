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

use crate::artifacts::ArtifactId;
use parity_scale_codec::{Decode, Encode};
use polkadot_parachain::primitives::ValidationCodeHash;
use polkadot_primitives::vstaging::ExecutorParams;
use sp_core::blake2_256;
use std::{
	cmp::{Eq, PartialEq},
	fmt,
	sync::Arc,
	time::Duration,
};

#[cfg(test)]
use crate::host::tests::TEST_PREPARATION_TIMEOUT;

/// A struct that carries code of a parachain validation function, its hash, a corresponding
/// set of executor parameters, and a preparation timeout.
///
/// Should be cheap to clone.
#[derive(Clone, Encode, Decode)]
pub struct PvfExhaustive {
	pub(crate) code: Arc<Vec<u8>>,
	pub(crate) code_hash: ValidationCodeHash,
	pub(crate) executor_params: Arc<ExecutorParams>,
	pub(crate) timeout: Duration,
}

impl PvfExhaustive {
	/// Returns an instance of the PVF out of the given PVF code and executor params.
	pub fn from_code(code: Vec<u8>, executor_params: ExecutorParams, timeout: Duration) -> Self {
		let code = Arc::new(code);
		let code_hash = blake2_256(&code).into();
		let executor_params = Arc::new(executor_params);
		Self { code, code_hash, executor_params, timeout }
	}

	/// Returns artifact ID that corresponds to the PVF with given executor params
	pub(crate) fn as_artifact_id(&self) -> ArtifactId {
		ArtifactId::new(self.code_hash, self.executor_params.hash())
	}

	/// Returns validation code hash for the PVF
	pub(crate) fn code_hash(&self) -> ValidationCodeHash {
		self.code_hash
	}

	/// Returns PVF code
	pub(crate) fn code(&self) -> Arc<Vec<u8>> {
		self.code.clone()
	}

	/// Returns executor params
	pub(crate) fn executor_params(&self) -> Arc<ExecutorParams> {
		self.executor_params.clone()
	}

	/// Returns preparation timeout
	pub(crate) fn timeout(&self) -> Duration {
		self.timeout
	}

	/// Creates a structure for tests
	#[cfg(test)]
	pub(crate) fn from_discriminator_and_timeout(num: u32, timeout: Duration) -> Self {
		let descriminator_buf = num.to_le_bytes().to_vec();
		Self::from_code(descriminator_buf, ExecutorParams::default(), timeout)
	}

	#[cfg(test)]
	pub(crate) fn from_discriminator(num: u32) -> Self {
		Self::from_discriminator_and_timeout(num, TEST_PREPARATION_TIMEOUT)
	}
}

impl fmt::Debug for PvfExhaustive {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"Pvf {{ code, code_hash: {:?}, executor_params: {:?} }}",
			self.code_hash, self.executor_params
		)
	}
}

impl PartialEq for PvfExhaustive {
	fn eq(&self, other: &Self) -> bool {
		self.code_hash == other.code_hash &&
			self.executor_params.hash() == other.executor_params.hash()
	}
}

impl Eq for PvfExhaustive {}
