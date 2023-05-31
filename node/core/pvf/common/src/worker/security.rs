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

//! Functionality for securing workers.
//!
//! This is needed because workers are used to compile and execute untrusted code (PVFs).

/// The	[landlock] docs say it best:
///
/// > "Landlock is a security feature available since Linux 5.13. The goal is to enable to restrict
/// ambient rights (e.g., global filesystem access) for a set of processes by creating safe security
/// sandboxes as new security layers in addition to the existing system-wide access-controls. This
/// kind of sandbox is expected to help mitigate the security impact of bugs, unexpected or
/// malicious behaviors in applications. Landlock empowers any process, including unprivileged ones,
/// to securely restrict themselves."
///
/// [landlock]: https://docs.rs/landlock/latest/landlock/index.html
#[cfg(target_os = "linux")]
pub mod landlock {
	// Export for checking the status.
	pub use landlock::RulesetStatus;

	use landlock::{Access, AccessFs, Ruleset, RulesetAttr, RulesetError, ABI};

	/// Version of landlock ABI. Use the latest version supported by our reference kernel version.
	///
	/// - Reference kernel version: 5.16+
	/// - ABI V1: 5.13
	/// - ABI V2: 5.19
	///
	/// Please update these docs if they are out-of-date, and bump the ABI version to the minimum
	/// ABI supported by the reference kernel version.
	pub const LANDLOCK_ABI: ABI = ABI::V1;

	// TODO: <https://github.com/landlock-lsm/rust-landlock/issues/36>
	/// Returns to what degree landlock is enabled on the current Linux environment.
	pub fn get_status() -> Result<RulesetStatus, Box<dyn std::error::Error>> {
		match std::thread::spawn(|| try_restrict_thread()).join() {
			Ok(Ok(status)) => Ok(status),
			Ok(Err(ruleset_err)) => Err(ruleset_err.into()),
			Err(_err) => Err("a panic occurred in try_restrict_thread".into()),
		}
	}

	/// Returns a single bool indicating whether landlock is fully enabled on the current Linux
	/// environment.
	pub fn is_fully_enabled() -> bool {
		matches!(get_status(), Ok(RulesetStatus::FullyEnforced))
	}

	/// Tries to restrict the current thread with the following landlock access controls:
	///
	/// 1. all global filesystem access
	/// 2. ... more may be supported in the future.
	///
	/// If landlock is not supported in the current environment this is simply a noop.
	///
	/// # Returns
	///
	/// The status of the restriction (whether it was fully, partially, or not-at-all enforced).
	pub fn try_restrict_thread() -> Result<RulesetStatus, RulesetError> {
		let status = Ruleset::new()
			.handle_access(AccessFs::from_all(LANDLOCK_ABI))?
			.create()?
			.restrict_self()?;
		Ok(status.ruleset)
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use std::{fs, io::ErrorKind, thread};

		#[test]
		fn restricted_thread_cannot_access_fs() {
			// TODO: This would be nice: <https://github.com/rust-lang/rust/issues/68007>.
			if !is_fully_enabled() {
				return
			}

			// Restricted thread cannot read from FS.
			let handle = thread::spawn(|| {
				// Write to a tmp file, this should succeed before landlock is applied.
				let text = "foo";
				let tmpfile = tempfile::NamedTempFile::new().unwrap();
				let path = tmpfile.path();
				fs::write(path, text).unwrap();
				let s = fs::read_to_string(path).unwrap();
				assert_eq!(s, text);

				let status = super::try_restrict_thread().unwrap();
				if let RulesetStatus::NotEnforced = status {
					panic!("Ruleset should be enforced since we checked if landlock is enabled");
				}

				// Try to read from the tmp file after landlock.
				let result = fs::read_to_string(path);
				assert!(matches!(
					result,
					Err(err) if matches!(err.kind(), ErrorKind::PermissionDenied)
				));
			});

			assert!(handle.join().is_ok());

			// Restricted thread cannot write to FS.
			let handle = thread::spawn(|| {
				let text = "foo";
				let tmpfile = tempfile::NamedTempFile::new().unwrap();
				let path = tmpfile.path();

				let status = super::try_restrict_thread().unwrap();
				if let RulesetStatus::NotEnforced = status {
					panic!("Ruleset should be enforced since we checked if landlock is enabled");
				}

				// Try to write to the tmp file after landlock.
				let result = fs::write(path, text);
				assert!(matches!(
					result,
					Err(err) if matches!(err.kind(), ErrorKind::PermissionDenied)
				));
			});

			assert!(handle.join().is_ok());
		}
	}
}
