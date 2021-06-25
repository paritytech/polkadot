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

use always_assert::always;
use async_std::{
	path::{Path, PathBuf},
};
use polkadot_parachain::primitives::ValidationCodeHash;
use std::{
	collections::HashMap,
	time::{Duration, SystemTime},
};
use parity_scale_codec::{Encode, Decode};

/// A final product of preparation process. Contains either a ready to run compiled artifact or
/// a description what went wrong.
#[derive(Encode, Decode)]
pub enum Artifact {
	/// During the prevalidation stage of preparation an issue was found with the PVF.
	PrevalidationErr(String),
	/// Compilation failed for the given PVF.
	PreparationErr(String),
	/// This state indicates that the process assigned to prepare the artifact wasn't responsible
	/// or were killed. This state is reported by the validation host (not by the worker).
	DidntMakeIt,
	/// The PVF passed all the checks and is ready for execution.
	Compiled { compiled_artifact: Vec<u8> },
}

impl Artifact {
	/// Serializes this struct into a byte buffer.
	pub fn serialize(&self) -> Vec<u8> {
		self.encode()
	}

	/// Deserialize the given byte buffer to an artifact.
	pub fn deserialize(mut bytes: &[u8]) -> Result<Self, String> {
		Artifact::decode(&mut bytes).map_err(|e| format!("{:?}", e))
	}
}

/// Identifier of an artifact. Right now it only encodes a code hash of the PVF. But if we get to
/// multiple engine implementations the artifact ID should include the engine type as well.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactId {
	code_hash: ValidationCodeHash,
}

impl ArtifactId {
	const PREFIX: &'static str = "wasmtime_";

	/// Creates a new artifact ID with the given hash.
	pub fn new(code_hash: ValidationCodeHash) -> Self {
		Self { code_hash }
	}

	/// Tries to recover the artifact id from the given file name.
	#[cfg(test)]
	pub fn from_file_name(file_name: &str) -> Option<Self> {
		use std::str::FromStr as _;
		use polkadot_core_primitives::Hash;

		let file_name = file_name.strip_prefix(Self::PREFIX)?;
		let code_hash = Hash::from_str(file_name).ok()?.into();

		Some(Self { code_hash })
	}

	/// Returns the expected path to this artifact given the root of the cache.
	pub fn path(&self, cache_path: &Path) -> PathBuf {
		let file_name = format!("{}{:#x}", Self::PREFIX, self.code_hash);
		cache_path.join(file_name)
	}
}

pub enum ArtifactState {
	/// The artifact is ready to be used by the executor.
	///
	/// That means that the artifact should be accessible through the path obtained by the artifact
	/// id (unless, it was removed externally).
	Prepared {
		/// The time when the artifact was the last time needed.
		///
		/// This is updated when we get the heads up for this artifact or when we just discover
		/// this file.
		last_time_needed: SystemTime,
	},
	/// A task to prepare this artifact is scheduled.
	Preparing,
}

/// A container of all known artifact ids and their states.
pub struct Artifacts {
	artifacts: HashMap<ArtifactId, ArtifactState>,
}

impl Artifacts {
	/// Initialize a blank cache at the given path. This will clear everything present at the
	/// given path, to be populated over time.
	///
	/// The recognized artifacts will be filled in the table and unrecognized will be removed.
	pub async fn new(cache_path: &Path) -> Self {
		// Make sure that the cache path directory and all it's parents are created.
		// First delete the entire cache. Nodes are long-running so this should populate shortly.
		let _ = async_std::fs::remove_dir_all(cache_path).await;
		let _ = async_std::fs::create_dir_all(cache_path).await;

		Self { artifacts: HashMap::new() }
	}

	#[cfg(test)]
	pub(crate) fn empty() -> Self {
		Self {
			artifacts: HashMap::new(),
		}
	}

	/// Returns the state of the given artifact by its ID.
	pub fn artifact_state_mut(&mut self, artifact_id: &ArtifactId) -> Option<&mut ArtifactState> {
		self.artifacts.get_mut(artifact_id)
	}

	/// Inform the table about the artifact with the given ID. The state will be set to "preparing".
	///
	/// This function must be used only for brand new artifacts and should never be used for
	/// replacing existing ones.
	pub fn insert_preparing(&mut self, artifact_id: ArtifactId) {
		// See the precondition.
		always!(self
			.artifacts
			.insert(artifact_id, ArtifactState::Preparing)
			.is_none());
	}

	/// Insert an artifact with the given ID as "prepared".
	///
	/// This function must be used only for brand new artifacts and should never be used for
	/// replacing existing ones.
	#[cfg(test)]
	pub fn insert_prepared(&mut self, artifact_id: ArtifactId, last_time_needed: SystemTime) {
		// See the precondition.
		always!(self
			.artifacts
			.insert(artifact_id, ArtifactState::Prepared { last_time_needed })
			.is_none());
	}

	/// Remove and retrieve the artifacts from the table that are older than the supplied Time-To-Live.
	pub fn prune(&mut self, artifact_ttl: Duration) -> Vec<ArtifactId> {
		let now = SystemTime::now();

		let mut to_remove = vec![];
		for (k, v) in self.artifacts.iter() {
			if let ArtifactState::Prepared {
				last_time_needed, ..
			} = *v {
				if now
					.duration_since(last_time_needed)
					.map(|age| age > artifact_ttl)
					.unwrap_or(false)
				{
					to_remove.push(k.clone());
				}
			}
		}

		for artifact in &to_remove {
			self.artifacts.remove(artifact);
		}

		to_remove
	}
}

#[cfg(test)]
mod tests {
	use async_std::path::Path;
	use super::{Artifacts, ArtifactId};
	use sp_core::H256;
	use std::str::FromStr;

	#[test]
	fn from_file_name() {
		assert!(ArtifactId::from_file_name("").is_none());
		assert!(ArtifactId::from_file_name("junk").is_none());

		assert_eq!(
			ArtifactId::from_file_name(
				"wasmtime_0x0022800000000000000000000000000000000000000000000000000000000000"
			),
			Some(ArtifactId::new(
				hex_literal::hex![
					"0022800000000000000000000000000000000000000000000000000000000000"
				]
				.into()
			)),
		);
	}

	#[test]
	fn path() {
		let path = Path::new("/test");
		let hash = H256::from_str("1234567890123456789012345678901234567890123456789012345678901234").unwrap().into();

		assert_eq!(
			ArtifactId::new(hash).path(path).to_str(),
			Some("/test/wasmtime_0x1234567890123456789012345678901234567890123456789012345678901234"),
		);
	}

	#[test]
	fn artifacts_removes_cache_on_startup() {
		let fake_cache_path = async_std::task::block_on(async move { crate::worker_common::tmpfile("test-cache").await.unwrap() });
		let fake_artifact_path = {
			let mut p = fake_cache_path.clone();
			p.push("wasmtime_0x1234567890123456789012345678901234567890123456789012345678901234");
			p
		};

		// create a tmp cache with 1 artifact.

		std::fs::create_dir_all(&fake_cache_path).unwrap();
		std::fs::File::create(fake_artifact_path).unwrap();

		// this should remove it and re-create.

		let p = &fake_cache_path;
		async_std::task::block_on(async { Artifacts::new(p).await });

		assert_eq!(std::fs::read_dir(&fake_cache_path).unwrap().count(), 0);

		std::fs::remove_dir_all(fake_cache_path).unwrap();
	}
}
