// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

use parity_scale_codec::{Decode, Encode};

/// Timestamp based on the 1 Jan 1970 UNIX base, which is persistent across node restarts and OS reboots.
pub type Timestamp = u64;

/// The status of dispute.
///
/// As managed by the dispute coordinator.
///
/// NOTE: This status is persisted to the database, any changes have to be versioned and a db
/// migration will be needed.
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
	/// Dispute has been confirmed (more than `byzantine_threshold` have already participated/ or
	/// we have seen the candidate included already/participated successfully ourselves).
	#[codec(index = 3)]
	Confirmed,
}

impl DisputeStatus {
	/// Initialize the status to the active state.
	pub fn active() -> DisputeStatus {
		DisputeStatus::Active
	}

	/// Move status to confirmed status, if not yet concluded/confirmed already.
	pub fn confirm(self) -> DisputeStatus {
		match self {
			DisputeStatus::Active => DisputeStatus::Confirmed,
			DisputeStatus::Confirmed => DisputeStatus::Confirmed,
			DisputeStatus::ConcludedFor(_) | DisputeStatus::ConcludedAgainst(_) => self,
		}
	}

	/// Check whether the dispute is not a spam dispute.
	pub fn is_confirmed_concluded(&self) -> bool {
		match self {
			&DisputeStatus::Confirmed |
			&DisputeStatus::ConcludedFor(_) |
			DisputeStatus::ConcludedAgainst(_) => true,
			&DisputeStatus::Active => false,
		}
	}

	/// Concluded valid?
	pub fn has_concluded_for(&self) -> bool {
		match self {
			&DisputeStatus::ConcludedFor(_) => true,
			_ => false,
		}
	}
	/// Concluded invalid?
	pub fn has_concluded_against(&self) -> bool {
		match self {
			&DisputeStatus::ConcludedAgainst(_) => true,
			_ => false,
		}
	}

	/// Transition the status to a new status after observing the dispute has concluded for the candidate.
	/// This may be a no-op if the status was already concluded.
	pub fn conclude_for(self, now: Timestamp) -> DisputeStatus {
		match self {
			DisputeStatus::Active | DisputeStatus::Confirmed => DisputeStatus::ConcludedFor(now),
			DisputeStatus::ConcludedFor(at) => DisputeStatus::ConcludedFor(std::cmp::min(at, now)),
			against => against,
		}
	}

	/// Transition the status to a new status after observing the dispute has concluded against the candidate.
	/// This may be a no-op if the status was already concluded.
	pub fn conclude_against(self, now: Timestamp) -> DisputeStatus {
		match self {
			DisputeStatus::Active | DisputeStatus::Confirmed =>
				DisputeStatus::ConcludedAgainst(now),
			DisputeStatus::ConcludedFor(at) =>
				DisputeStatus::ConcludedAgainst(std::cmp::min(at, now)),
			DisputeStatus::ConcludedAgainst(at) =>
				DisputeStatus::ConcludedAgainst(std::cmp::min(at, now)),
		}
	}

	/// Whether the disputed candidate is possibly invalid.
	pub fn is_possibly_invalid(&self) -> bool {
		match self {
			DisputeStatus::Active |
			DisputeStatus::Confirmed |
			DisputeStatus::ConcludedAgainst(_) => true,
			DisputeStatus::ConcludedFor(_) => false,
		}
	}

	/// Yields the timestamp this dispute concluded at, if any.
	pub fn concluded_at(&self) -> Option<Timestamp> {
		match self {
			DisputeStatus::Active | DisputeStatus::Confirmed => None,
			DisputeStatus::ConcludedFor(at) | DisputeStatus::ConcludedAgainst(at) => Some(*at),
		}
	}
}

/// The choice here is fairly arbitrary. But any dispute that concluded more than a few minutes ago
/// is not worth considering anymore. Changing this value has little to no bearing on consensus,
/// and really only affects the work that the node might do on startup during periods of many
/// disputes.
pub const ACTIVE_DURATION_SECS: Timestamp = 180;

/// Returns true if the dispute has concluded for longer than [`ACTIVE_DURATION_SECS`].
pub fn dispute_is_inactive(status: &DisputeStatus, now: &Timestamp) -> bool {
	let at = status.concluded_at();

	at.is_some() && at.unwrap() + ACTIVE_DURATION_SECS < *now
}
