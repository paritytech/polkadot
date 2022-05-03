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

use crate::cache::OverlayCache;

use super::db::v1::{CandidateVotes, RecentDisputes};
use crate::{error::FatalResult, metrics::Metrics};

#[derive(Debug, PartialEq)]
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
	cache: &'a mut OverlayCache,
	// Reports read operations.
	metrics: Metrics,
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	pub fn new(backend: &'a B, metrics: Metrics, cache: &'a mut OverlayCache) -> Self {
		Self { inner: backend, cache, metrics }
	}

	/// Load the earliest session, if any.
	pub fn load_earliest_session(&self) -> SubsystemResult<Option<SessionIndex>> {
		let _read_timer = self.metrics.time_db_read_operation("earliest_session");
		if let Some(val) = self.cache.get_earliest_session() {
			return Ok(Some(*val))
		}

		self.inner.load_earliest_session()
	}

	/// Load the recent disputes, if any.
	pub fn load_recent_disputes(&mut self) -> SubsystemResult<Option<RecentDisputes>> {
		let _read_timer = self.metrics.time_db_read_operation("recent_disputes");
		if let Some(disputes) = self.cache.load_recent_disputes() {
			return Ok(Some(disputes))
		}

		let disputes = self.inner.load_recent_disputes()?;

		if let Some(disputes) = disputes {
			self.cache.update_recent_disputes(disputes.clone());
			Ok(Some(disputes))
		} else {
			Ok(None)
		}
	}

	/// Clone the candidate votes for the specific session-candidate pair, if any.
	pub fn load_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateVotes>> {
		let _read_timer = self.metrics.time_db_read_operation("candidate_votes");
		if let Some(candidate_votes) = self.cache.load_candidate_votes(session, candidate_hash) {
			return Ok(candidate_votes)
		}

		let votes = self.inner.load_candidate_votes(session, candidate_hash)?;

		if let Some(votes) = votes {
			self.cache.update_candidate_votes(session, candidate_hash, Some(votes.clone()));
			Ok(Some(votes))
		} else {
			Ok(None)
		}
	}
	/// Prepare a write to the "earliest session" field of the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub fn write_earliest_session(&mut self, session: SessionIndex) {
		self.cache.update_earliest_sesion(session);
	}

	/// Prepare a write to the recent disputes stored in the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub fn write_recent_disputes(&mut self, recent_disputes: RecentDisputes) {
		self.cache.update_recent_disputes(recent_disputes);
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
		self.cache.update_candidate_votes(session, &candidate_hash, Some(votes));
	}

	/// Prepare a deletion of the candidate votes under the indicated candidate.
	///
	/// Later calls to this function for the same candidate will override earlier ones.
	pub fn delete_candidate_votes(&mut self, session: SessionIndex, candidate_hash: CandidateHash) {
		// This will dirty the cache entry, not actually remove it.
		self.cache.update_candidate_votes(session, &candidate_hash, None);
	}
}
