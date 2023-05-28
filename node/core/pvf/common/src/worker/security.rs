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

#[cfg(target_os = "linux")]
pub mod landlock {
	use landlock::{
		path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr,
		RulesetError, RulesetStatus, ABI,
	};

	/// Returns to what degree landlock is enabled on the current Linux environment.
	///
	/// This is a separate check so it can run outside a worker thread, so the results can be sent via telemetry.
	pub fn check_enabled() {}

	/// Tries to restrict the current thread with the following landlock access controls:
	///
	/// 1. all global filesystem access
	/// 2. ... more may be supported in the future.
	pub fn try_restrict_thread() -> Result<RulesetStatus, RulesetError> {
		let abi = ABI::V2;
		let status = Ruleset::new()
			.handle_access(AccessFs::from_all(abi))?
			.create()?
			.restrict_self()?;
		Ok(status.ruleset)
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use crate::worker::thread::{self, WaitOutcome};
		use assert_matches::assert_matches;
		use std::sync::Arc;

		#[test]
		fn restricted_thread_cannot_access_fs() {
			// This would be nice: <https://github.com/rust-lang/rust/issues/68007>.
			if !check_enabled() {
				return
			}

			let cond = thread::get_condvar();

			// Use the same method we use to spawn workers in production.
			let handle = thread::spawn_worker_thread(
				"test_restricted_thread_cannot_access_fs",
				|| true,
				Arc::clone(&cond),
				WaitOutcome::Finished,
			)
			.unwrap();

			let outcome = thread::wait_for_threads(cond);

			assert_matches!(outcome, WaitOutcome::Finished);
			assert_matches!(handle.join(), Ok(true));
		}
	}
}
