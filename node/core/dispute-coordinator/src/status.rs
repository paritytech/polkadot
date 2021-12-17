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

use std::time::{SystemTime, UNIX_EPOCH};

use parity_scale_codec::{Decode, Encode};

use crate::LOG_TARGET;

/// The choice here is fairly arbitrary. But any dispute that concluded more than a few minutes ago
/// is not worth considering anymore. Changing this value has little to no bearing on consensus,
/// and really only affects the work that the node might do on startup during periods of many
/// disputes.
pub const ACTIVE_DURATION_SECS: Timestamp = 180;

/// Timestamp based on the 1 Jan 1970 UNIX base, which is persistent across node restarts and OS reboots.
pub type Timestamp = u64;

/// The status of dispute. This is a state machine which can be altered by the
/// helper methods.
#[derive(Debug, Clone, Copy, Encode, Decode, PartialEq)]
pub enum DisputeStatus {
	/// The dispute is active and unconcluded.
	#[codec(index = 0)]
	Active,
	/// The dispute has been concluded in favor of the candidate
	/// since the given timestamp.
	#[codec(index = 1)]
	ConcludedFor(Timestamp),
	/// The dispute has been concluded against the candidate
	/// since the given timestamp.
	///
	/// This takes precedence over `ConcludedFor` in the case that
	/// both are true, which is impossible unless a large amount of
	/// validators are participating on both sides.
	#[codec(index = 2)]
	ConcludedAgainst(Timestamp),
}

impl DisputeStatus {
	/// Initialize the status to the active state.
	pub fn active() -> DisputeStatus {
		DisputeStatus::Active
	}

	/// Transition the status to a new status after observing the dispute has concluded for the candidate.
	/// This may be a no-op if the status was already concluded.
	pub fn concluded_for(self, now: Timestamp) -> DisputeStatus {
		match self {
			DisputeStatus::Active => DisputeStatus::ConcludedFor(now),
			DisputeStatus::ConcludedFor(at) => DisputeStatus::ConcludedFor(std::cmp::min(at, now)),
			against => against,
		}
	}

	/// Transition the status to a new status after observing the dispute has concluded against the candidate.
	/// This may be a no-op if the status was already concluded.
	pub fn concluded_against(self, now: Timestamp) -> DisputeStatus {
		match self {
			DisputeStatus::Active => DisputeStatus::ConcludedAgainst(now),
			DisputeStatus::ConcludedFor(at) =>
				DisputeStatus::ConcludedAgainst(std::cmp::min(at, now)),
			DisputeStatus::ConcludedAgainst(at) =>
				DisputeStatus::ConcludedAgainst(std::cmp::min(at, now)),
		}
	}

	/// Whether the disputed candidate is possibly invalid.
	pub fn is_possibly_invalid(&self) -> bool {
		match self {
			DisputeStatus::Active | DisputeStatus::ConcludedAgainst(_) => true,
			DisputeStatus::ConcludedFor(_) => false,
		}
	}

	/// Yields the timestamp this dispute concluded at, if any.
	pub fn concluded_at(&self) -> Option<Timestamp> {
		match self {
			DisputeStatus::Active => None,
			DisputeStatus::ConcludedFor(at) | DisputeStatus::ConcludedAgainst(at) => Some(*at),
		}
	}
}

pub(crate) trait Clock: Send + Sync {
	fn now(&self) -> Timestamp;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
	fn now(&self) -> Timestamp {
		// `SystemTime` is notoriously non-monotonic, so our timers might not work
		// exactly as expected.
		//
		// Regardless, disputes are considered active based on an order of minutes,
		// so a few seconds of slippage in either direction shouldn't affect the
		// amount of work the node is doing significantly.
		match SystemTime::now().duration_since(UNIX_EPOCH) {
			Ok(d) => d.as_secs(),
			Err(e) => {
				tracing::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Current time is before unix epoch. Validation will not work correctly."
				);

				0
			},
		}
	}
}
