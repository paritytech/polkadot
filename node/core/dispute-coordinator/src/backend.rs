// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! An abstraction over storage used by the chain selection subsystem.
//!
//! This provides both a [`Backend`] trait and an [`OverlayedBackend`]
//! struct which allows in-memory changes to be applied on top of a
//! [`Backend`], maintaining consistency between queries and temporary writes,
//! before any commit to the underlying storage is made.

use polkadot_node_subsystem::SubsystemResult;
use polkadot_primitives::v2::{CandidateHash, SessionIndex};

use std::collections::HashMap;

use super::db::v1::{CandidateVotes, RecentDisputes};
use crate::error::FatalResult;

#[derive(Debug)]
pub enum BackendWriteOp {
	WriteEarliestSession(SessionIndex),
	WriteRecentDisputes(RecentDisputes),
	WriteCandidateVotes(SessionIndex, CandidateHash, CandidateVotes),
	DeleteCandidateVotes(SessionIndex, CandidateHash),
}

/// An abstraction over backend storage for the logic of this subsystem.
pub trait Backend {
	/// Load the earliest session, if any.
	fn load_earliest_session(&self) -> SubsystemResult<Option<SessionIndex>>;

	/// Load the recent disputes, if any.
	fn load_recent_disputes(&self) -> SubsystemResult<Option<RecentDisputes>>;

	/// Load the candidate votes for the specific session-candidate pair, if any.
	fn load_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateVotes>>;

	/// Atomically writes the list of operations, with later operations taking precedence over
	/// prior.
	fn write<I>(&mut self, ops: I) -> FatalResult<()>
	where
		I: IntoIterator<Item = BackendWriteOp>;
}

/// An in-memory overlay for the backend.
///
/// This maintains read-only access to the underlying backend, but can be converted into a set of
/// write operations which will, when written to the underlying backend, give the same view as the
/// state of the overlay.
pub struct OverlayedBackend<'a, B: 'a> {
	inner: &'a B,

	// `None` means unchanged.
	earliest_session: Option<SessionIndex>,
	// `None` means unchanged.
	recent_disputes: Option<RecentDisputes>,
	// `None` means deleted, missing means query inner.
	candidate_votes: HashMap<(SessionIndex, CandidateHash), Option<CandidateVotes>>,
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	pub fn new(backend: &'a B) -> Self {
		Self {
			inner: backend,
			earliest_session: None,
			recent_disputes: None,
			candidate_votes: HashMap::new(),
		}
	}

	/// Returns true if the are no write operations to perform.
	pub fn is_empty(&self) -> bool {
		self.earliest_session.is_none() &&
			self.recent_disputes.is_none() &&
			self.candidate_votes.is_empty()
	}

	/// Load the earliest session, if any.
	pub fn load_earliest_session(&self) -> SubsystemResult<Option<SessionIndex>> {
		if let Some(val) = self.earliest_session {
			return Ok(Some(val))
		}

		self.inner.load_earliest_session()
	}

	/// Load the recent disputes, if any.
	pub fn load_recent_disputes(&self) -> SubsystemResult<Option<RecentDisputes>> {
		if let Some(val) = &self.recent_disputes {
			return Ok(Some(val.clone()))
		}

		self.inner.load_recent_disputes()
	}

	/// Load the candidate votes for the specific session-candidate pair, if any.
	pub fn load_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateVotes>> {
		if let Some(val) = self.candidate_votes.get(&(session, *candidate_hash)) {
			return Ok(val.clone())
		}

		self.inner.load_candidate_votes(session, candidate_hash)
	}

	/// Prepare a write to the "earliest session" field of the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub fn write_earliest_session(&mut self, session: SessionIndex) {
		self.earliest_session = Some(session);
	}

	/// Prepare a write to the recent disputes stored in the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub fn write_recent_disputes(&mut self, recent_disputes: RecentDisputes) {
		self.recent_disputes = Some(recent_disputes)
	}

	/// Prepare a write of the candidate votes under the indicated candidate.
	///
	/// Later calls to this function for the same candidate will override earlier ones.
	pub fn write_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: CandidateHash,
		votes: CandidateVotes,
	) {
		self.candidate_votes.insert((session, candidate_hash), Some(votes));
	}

	/// Transform this backend into a set of write-ops to be written to the inner backend.
	pub fn into_write_ops(self) -> impl Iterator<Item = BackendWriteOp> {
		let earliest_session_ops = self
			.earliest_session
			.map(|s| BackendWriteOp::WriteEarliestSession(s))
			.into_iter();

		let recent_dispute_ops =
			self.recent_disputes.map(|d| BackendWriteOp::WriteRecentDisputes(d)).into_iter();

		let candidate_vote_ops =
			self.candidate_votes
				.into_iter()
				.map(|((session, candidate), votes)| match votes {
					Some(votes) => BackendWriteOp::WriteCandidateVotes(session, candidate, votes),
					None => BackendWriteOp::DeleteCandidateVotes(session, candidate),
				});

		earliest_session_ops.chain(recent_dispute_ops).chain(candidate_vote_ops)
	}
}
