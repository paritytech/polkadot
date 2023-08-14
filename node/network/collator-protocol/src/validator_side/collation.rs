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

//! Primitives for tracking collations-related data.
//!
//! Usually a path of collations is as follows:
//!    1. First, collation must be advertised by collator.
//!    2. If the advertisement was accepted, it's queued for fetch (per relay parent).
//!    3. Once it's requested, the collation is said to be Pending.
//!    4. Pending collation becomes Fetched once received, we send it to backing for validation.
//!    5. If it turns to be invalid or async backing allows seconding another candidate, carry on
//!       with the next advertisement, otherwise we're done with this relay parent.
//!
//!    ┌──────────────────────────────────────────┐
//!    └─▶Advertised ─▶ Pending ─▶ Fetched ─▶ Validated

use std::{collections::VecDeque, future::Future, pin::Pin, task::Poll};

use futures::{future::BoxFuture, FutureExt};
use polkadot_node_network_protocol::{
	request_response::{outgoing::RequestError, v1 as request_v1, OutgoingResult},
	PeerId,
};
use polkadot_node_primitives::PoV;
use polkadot_node_subsystem::jaeger;
use polkadot_node_subsystem_util::{
	metrics::prometheus::prometheus::HistogramTimer, runtime::ProspectiveParachainsMode,
};
use polkadot_primitives::{
	CandidateHash, CandidateReceipt, CollatorId, Hash, Id as ParaId, PersistedValidationData,
};
use tokio_util::sync::CancellationToken;

use crate::{error::SecondingError, LOG_TARGET};

/// Candidate supplied with a para head it's built on top of.
#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct ProspectiveCandidate {
	/// Candidate hash.
	pub candidate_hash: CandidateHash,
	/// Parent head-data hash as supplied in advertisement.
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
	/// Candidate's relay parent.
	pub relay_parent: Hash,
	/// Parachain id.
	pub para_id: ParaId,
	/// Candidate hash.
	pub candidate_hash: CandidateHash,
	/// Id of the collator the collation was fetched from.
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

/// Identifier of a collation being requested.
#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct PendingCollation {
	/// Candidate's relay parent.
	pub relay_parent: Hash,
	/// Parachain id.
	pub para_id: ParaId,
	/// Peer that advertised this collation.
	pub peer_id: PeerId,
	/// Optional candidate hash and parent head-data hash if were
	/// supplied in advertisement.
	pub prospective_candidate: Option<ProspectiveCandidate>,
	/// Hash of the candidate's commitments.
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
			peer_id: *peer_id,
			prospective_candidate,
			commitments_hash: None,
		}
	}
}

/// vstaging advertisement that was rejected by the backing
/// subsystem. Validator may fetch it later if its fragment
/// membership gets recognized before relay parent goes out of view.
#[derive(Debug, Clone)]
pub struct BlockedAdvertisement {
	/// Peer that advertised the collation.
	pub peer_id: PeerId,
	/// Collator id.
	pub collator_id: CollatorId,
	/// The relay-parent of the candidate.
	pub candidate_relay_parent: Hash,
	/// Hash of the candidate.
	pub candidate_hash: CandidateHash,
}

/// Performs a sanity check between advertised and fetched collations.
///
/// Since the persisted validation data is constructed using the advertised
/// parent head data hash, the latter doesn't require an additional check.
pub fn fetched_collation_sanity_check(
	advertised: &PendingCollation,
	fetched: &CandidateReceipt,
	persisted_validation_data: &PersistedValidationData,
) -> Result<(), SecondingError> {
	if persisted_validation_data.hash() != fetched.descriptor().persisted_validation_data_hash {
		Err(SecondingError::PersistedValidationDataMismatch)
	} else if advertised
		.prospective_candidate
		.map_or(false, |pc| pc.candidate_hash() != fetched.hash())
	{
		Err(SecondingError::CandidateHashMismatch)
	} else {
		Ok(())
	}
}

/// Identifier for a requested collation and the respective collator that advertised it.
#[derive(Debug, Clone)]
pub struct CollationEvent {
	/// Collator id.
	pub collator_id: CollatorId,
	/// The requested collation data.
	pub pending_collation: PendingCollation,
}

/// Fetched collation data.
#[derive(Debug, Clone)]
pub struct PendingCollationFetch {
	/// Collation identifier.
	pub collation_event: CollationEvent,
	/// Candidate receipt.
	pub candidate_receipt: CandidateReceipt,
	/// Proof of validity.
	pub pov: PoV,
}

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
	/// Collator we're fetching from, optionally which candidate was requested.
	///
	/// This is the currently last started fetch, which did not exceed `MAX_UNSHARED_DOWNLOAD_TIME`
	/// yet.
	pub fetching_from: Option<(CollatorId, Option<CandidateHash>)>,
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

	/// Returns the next collation to fetch from the `waiting_queue`.
	///
	/// This will reset the status back to `Waiting` using [`CollationStatus::back_to_waiting`].
	///
	/// Returns `Some(_)` if there is any collation to fetch, the `status` is not `Seconded` and
	/// the passed in `finished_one` is the currently `waiting_collation`.
	pub(super) fn get_next_collation_to_fetch(
		&mut self,
		finished_one: &(CollatorId, Option<CandidateHash>),
		relay_parent_mode: ProspectiveParachainsMode,
	) -> Option<(PendingCollation, CollatorId)> {
		// If finished one does not match waiting_collation, then we already dequeued another fetch
		// to replace it.
		if let Some((collator_id, maybe_candidate_hash)) = self.fetching_from.as_ref() {
			// If a candidate hash was saved previously, `finished_one` must include this too.
			if collator_id != &finished_one.0 &&
				maybe_candidate_hash.map_or(true, |hash| Some(&hash) != finished_one.1.as_ref())
			{
				gum::trace!(
					target: LOG_TARGET,
					waiting_collation = ?self.fetching_from,
					?finished_one,
					"Not proceeding to the next collation - has already been done."
				);
				return None
			}
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
			if let ProspectiveParachainsMode::Enabled { max_candidate_depth, .. } =
				relay_parent_mode
			{
				max_candidate_depth + 1
			} else {
				1
			};
		self.seconded_count < seconded_limit
	}
}

// Any error that can occur when awaiting a collation fetch response.
#[derive(Debug, thiserror::Error)]
pub(super) enum CollationFetchError {
	#[error("Future was cancelled.")]
	Cancelled,
	#[error("{0}")]
	Request(#[from] RequestError),
}

/// Future that concludes when the collator has responded to our collation fetch request
/// or the request was cancelled by the validator.
pub(super) struct CollationFetchRequest {
	/// Info about the requested collation.
	pub pending_collation: PendingCollation,
	/// Collator id.
	pub collator_id: CollatorId,
	/// Responses from collator.
	pub from_collator: BoxFuture<'static, OutgoingResult<request_v1::CollationFetchingResponse>>,
	/// Handle used for checking if this request was cancelled.
	pub cancellation_token: CancellationToken,
	/// A jaeger span corresponding to the lifetime of the request.
	pub span: Option<jaeger::Span>,
	/// A metric histogram for the lifetime of the request
	pub _lifetime_timer: Option<HistogramTimer>,
}

impl Future for CollationFetchRequest {
	type Output = (
		CollationEvent,
		std::result::Result<request_v1::CollationFetchingResponse, CollationFetchError>,
	);

	fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
		// First check if this fetch request was cancelled.
		let cancelled = match std::pin::pin!(self.cancellation_token.cancelled()).poll(cx) {
			Poll::Ready(()) => true,
			Poll::Pending => false,
		};

		if cancelled {
			self.span.as_mut().map(|s| s.add_string_tag("success", "false"));
			return Poll::Ready((
				CollationEvent {
					collator_id: self.collator_id.clone(),
					pending_collation: self.pending_collation,
				},
				Err(CollationFetchError::Cancelled),
			))
		}

		let res = self.from_collator.poll_unpin(cx).map(|res| {
			(
				CollationEvent {
					collator_id: self.collator_id.clone(),
					pending_collation: self.pending_collation,
				},
				res.map_err(CollationFetchError::Request),
			)
		});

		match &res {
			Poll::Ready((_, Ok(request_v1::CollationFetchingResponse::Collation(..)))) => {
				self.span.as_mut().map(|s| s.add_string_tag("success", "true"));
			},
			Poll::Ready((_, Err(_))) => {
				self.span.as_mut().map(|s| s.add_string_tag("success", "false"));
			},
			_ => {},
		};

		res
	}
}
