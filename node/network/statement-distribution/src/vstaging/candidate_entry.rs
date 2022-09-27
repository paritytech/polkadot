// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! A [`CandidateEntry`] tracks all info concerning a candidate block.
//!
//! This entity doesn't actually store the statements about the candidate,
//! just metadata of which validators have seconded or validated the
//! candidate, and the candidate and [`PersistedValidationData`] itself,
//! if that has already been fetched.
//!
//! Note that it is possible for validators for multiple groups to second
//! a candidate. Given that each candidate's para and relay-parent is
//! determined by the candidate hash, and the current scheduling mechanism
//! of the relay-chain only schedules one group per para per relay-parent,
//! this is certainly in error. Nevertheless, if we receive statements about
//! a candidate _prior_ to fetching the candidate itself, we do not have
//! confirmation of which group is assigned to the para in actuality.

use polkadot_primitives::vstaging::{
	CandidateHash, CommittedCandidateReceipt, GroupIndex, PersistedValidationData,
};

use std::collections::HashMap;

/// A tracker for all validators which have seconded or validated a particular
/// candidate. See module docs for more details.
pub struct CandidateEntry {
	candidate_hash: CandidateHash,
	state: CandidateState,
}

impl CandidateEntry {
	/// Create a blank [`CandidateEntry`]
	pub fn new(candidate_hash: CandidateHash) -> Self {
		CandidateEntry { candidate_hash, state: CandidateState::Unconfirmed(HashMap::new()) }
	}

	/// Supply the [`CommittedCandidateReceipt`] and [`PersistedValidationData`].
	/// This does not check that the receipt matches the candidate hash nor that the PVD
	/// matches the commitment in the candidate's descriptor. Also, supply
	/// the intended [`GroupIndex`] assigned to the para at the relay-parent
	/// of the candidate-receipt.
	///
	/// No-op if already provided.
	pub fn confirm(
		&mut self,
		candidate: CommittedCandidateReceipt,
		pvd: PersistedValidationData,
		group_index: GroupIndex,
	) {
	}

	/// Whether the candidate is confirmed to actually exist.
	pub fn is_confirmed(&self) -> bool {
		match self.state {
			CandidateState::Unconfirmed(_) => false,
			CandidateState::Confirmed { .. } => true,
		}
	}

	/// The receipt of the candidate descriptor. Only exists if confirmed.
	pub fn receipt(&self) -> Option<&CommittedCandidateReceipt> {
		match self.state {
			CandidateState::Confirmed { ref candidate, .. } => Some(candidate),
			CandidateState::Unconfirmed(_) => None,
		}
	}
}

enum CandidateState {
	Unconfirmed(HashMap<GroupIndex, GroupAttestationRecord>),
	Confirmed {
		candidate: CommittedCandidateReceipt,
		persisted_validation_data: PersistedValidationData,
		group_index: GroupIndex,
		attestations: GroupAttestationRecord,
	},
}

struct GroupAttestationRecord {
	// TODO [now]
}
