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

use futures::channel::oneshot;
use std::collections::VecDeque;

use polkadot_node_network_protocol::PeerId;
use polkadot_node_primitives::PoV;
use polkadot_primitives::v2::{CandidateHash, CandidateReceipt, CollatorId, Hash, Id as ParaId};

use crate::{ProspectiveParachainsMode, LOG_TARGET, MAX_CANDIDATE_DEPTH};

/// Candidate supplied with a para head it's built on top of.
#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct ProspectiveCandidate {
	pub candidate_hash: CandidateHash,
	pub parent_head_data_hash: Hash,
}

impl ProspectiveCandidate {
	pub fn candidate_hash(&self) -> CandidateHash {
		self.candidate_hash
	}
}

/// Identifier of a fetched collation.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct FetchedCollation {
	pub relay_parent: Hash,
	pub para_id: ParaId,
	pub candidate_hash: CandidateHash,
	pub collator_id: CollatorId,
}

impl From<&CandidateReceipt<Hash>> for FetchedCollation {
	fn from(receipt: &CandidateReceipt<Hash>) -> Self {
		let descriptor = receipt.descriptor();
		Self {
			relay_parent: descriptor.relay_parent,
			para_id: descriptor.para_id,
			candidate_hash: receipt.hash(),
			collator_id: descriptor.collator.clone(),
		}
	}
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PendingCollation {
	pub relay_parent: Hash,
	pub para_id: ParaId,
	pub peer_id: PeerId,
	pub prospective_candidate: Option<ProspectiveCandidate>,
	pub commitments_hash: Option<Hash>,
}

impl PendingCollation {
	pub fn new(
		relay_parent: Hash,
		para_id: ParaId,
		peer_id: &PeerId,
		prospective_candidate: Option<ProspectiveCandidate>,
	) -> Self {
		Self {
			relay_parent,
			para_id,
			peer_id: peer_id.clone(),
			prospective_candidate,
			commitments_hash: None,
		}
	}
}

pub type CollationEvent = (CollatorId, PendingCollation);

pub type PendingCollationFetch =
	(CollationEvent, std::result::Result<(CandidateReceipt, PoV), oneshot::Canceled>);

/// The status of the collations in [`CollationsPerRelayParent`].
#[derive(Debug, Clone, Copy)]
pub enum CollationStatus {
	/// We are waiting for a collation to be advertised to us.
	Waiting,
	/// We are currently fetching a collation.
	Fetching,
	/// We are waiting that a collation is being validated.
	WaitingOnValidation,
	/// We have seconded a collation.
	Seconded,
}

impl Default for CollationStatus {
	fn default() -> Self {
		Self::Waiting
	}
}

impl CollationStatus {
	/// Downgrades to `Waiting`, but only if `self != Seconded`.
	fn back_to_waiting(&mut self, relay_parent_mode: ProspectiveParachainsMode) {
		match self {
			Self::Seconded =>
				if relay_parent_mode.is_enabled() {
					// With async backing enabled it's allowed to
					// second more candidates.
					*self = Self::Waiting
				},
			_ => *self = Self::Waiting,
		}
	}
}

/// Information about collations per relay parent.
#[derive(Default)]
pub struct Collations {
	/// What is the current status in regards to a collation for this relay parent?
	pub status: CollationStatus,
	/// Collator we're fetching from.
	///
	/// This is the currently last started fetch, which did not exceed `MAX_UNSHARED_DOWNLOAD_TIME`
	/// yet.
	pub fetching_from: Option<CollatorId>,
	/// Collation that were advertised to us, but we did not yet fetch.
	pub waiting_queue: VecDeque<(PendingCollation, CollatorId)>,
	/// How many collations have been seconded.
	pub seconded_count: usize,
}

impl Collations {
	/// Note a seconded collation for a given para.
	pub(super) fn note_seconded(&mut self) {
		self.seconded_count += 1
	}

	/// Returns the next collation to fetch from the `unfetched_collations`.
	///
	/// This will reset the status back to `Waiting` using [`CollationStatus::back_to_waiting`].
	///
	/// Returns `Some(_)` if there is any collation to fetch, the `status` is not `Seconded` and
	/// the passed in `finished_one` is the currently `waiting_collation`.
	pub(super) fn get_next_collation_to_fetch(
		&mut self,
		finished_one: Option<&CollatorId>,
		relay_parent_mode: ProspectiveParachainsMode,
	) -> Option<(PendingCollation, CollatorId)> {
		// If finished one does not match waiting_collation, then we already dequeued another fetch
		// to replace it.
		if self.fetching_from.as_ref() != finished_one {
			gum::trace!(
				target: LOG_TARGET,
				waiting_collation = ?self.fetching_from,
				?finished_one,
				"Not proceeding to the next collation - has already been done."
			);
			return None
		}
		self.status.back_to_waiting(relay_parent_mode);

		match self.status {
			// We don't need to fetch any other collation when we already have seconded one.
			CollationStatus::Seconded => None,
			CollationStatus::Waiting =>
				if !self.is_seconded_limit_reached(relay_parent_mode) {
					None
				} else {
					self.waiting_queue.pop_front()
				},
			CollationStatus::WaitingOnValidation | CollationStatus::Fetching =>
				unreachable!("We have reset the status above!"),
		}
	}

	/// Checks the limit of seconded candidates for a given para.
	pub(super) fn is_seconded_limit_reached(
		&self,
		relay_parent_mode: ProspectiveParachainsMode,
	) -> bool {
		let seconded_limit =
			if relay_parent_mode.is_enabled() { MAX_CANDIDATE_DEPTH + 1 } else { 1 };
		self.seconded_count < seconded_limit
	}
}
