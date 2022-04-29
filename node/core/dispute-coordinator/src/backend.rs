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

use lru_cache::LruCache;
use std::collections::HashMap;

use super::db::v1::{CandidateVotes, RecentDisputes};
use crate::{error::FatalResult, metrics::Metrics};

#[derive(Debug)]
pub enum BackendWriteOp<'a> {
	WriteEarliestSession(SessionIndex),
	WriteRecentDisputes(&'a RecentDisputes),
	WriteCandidateVotes(SessionIndex, CandidateHash, &'a CandidateVotes),
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
	fn write<'a, I>(&mut self, ops: I) -> FatalResult<()>
	where
		I: IntoIterator<Item = BackendWriteOp<'a>>;
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

/// An enum wrapper for changes that will be eventually saved in the DB.
#[derive(Clone)]
pub enum PersistedCacheEntry<T> {
	/// The entry is in memory and is unmodified. This is the initial state.
	Cached(T),
	/// The entry is in memory and was modified (needs to be persisted later).
	Dirty(T),
	/// The entry has been persisted in the DB.
	Persisted(T),
}

impl<T> PersistedCacheEntry<T> {
	pub fn new(inner: T) -> Self {
		Self::Cached(inner)
	}

	pub fn is_dirty(&self) -> bool {
		match self {
			Self::Dirty(_) => true,
			_ => false,
		}
	}

	/// Convenience method to get a ref to inner value. Avoids matching in the calling code.
	pub fn value(&self) -> &T {
		match self {
			Self::Cached(inner) => inner,
			Self::Dirty(inner) => inner,
			Self::Persisted(inner) => inner,
		}
	}

	pub fn into_inner(self) -> T {
		match self {
			Self::Cached(inner) => inner,
			Self::Dirty(inner) => inner,
			Self::Persisted(inner) => inner,
		}
	}
}

/// The LRU cache size for candidate votes.
const CACHE_SIZE: usize = 1024;
/// The write back interval in seconds
const WRITE_BACK_INTERVAL: usize = 30;

/// A cache (LRU) with write-back capabilities.
/// How caching works:
/// - Candidate votes are kept in memory in the `cached_candidate_votes` LRU cache.
/// - Once the cache reaches `CACHE_SIZE`, we start
///
/// How write-back works:
/// - The evicted entries must be collected via `into_write_ops()`, which returns an iterator
/// over the corresponding backend operations.
pub struct OverlayCache {
	cached_earliest_session: Option<PersistedCacheEntry<SessionIndex>>,
	cached_recent_disputes: Option<PersistedCacheEntry<RecentDisputes>>,
	cached_candidate_votes:
		LruCache<(SessionIndex, CandidateHash), PersistedCacheEntry<Option<CandidateVotes>>>,

	/// We need this to hold stuff that was evicted from the LRU cache up until the next `into_write_ops()` call.
	/// All entries here need to be `Dirty`.
	evicted_candidate_votes:
		HashMap<(SessionIndex, CandidateHash), PersistedCacheEntry<Option<CandidateVotes>>>,
	dirty: bool,
}

impl OverlayCache {
	pub fn new() -> Self {
		OverlayCache {
			cached_earliest_session: None,
			cached_recent_disputes: None,
			cached_candidate_votes: LruCache::new(CACHE_SIZE),
			evicted_candidate_votes: HashMap::new(),
			dirty: false,
		}
	}

	/// Initialize the earliest session.
	pub fn initialize_earliest_sesion(&mut self, earliest_session: SessionIndex) {
		self.cached_earliest_session = Some(PersistedCacheEntry::Cached(earliest_session));
	}

	/// Update the earliest session.
	pub fn update_earliest_sesion(&mut self, earliest_session: SessionIndex) {
		self.cached_earliest_session = Some(PersistedCacheEntry::Dirty(earliest_session));
		self.dirty = true;
	}

	/// Get the earliest session from the cache.
	pub fn get_earliest_session(&self) -> Option<&SessionIndex> {
		if let Some(cache_entry) = &self.cached_earliest_session {
			return Some(cache_entry.value())
		}
		None
	}

	/// Initialize the earliest session.
	pub fn initialize_recent_disputes(&mut self, recent_disputes: RecentDisputes) {
		self.cached_recent_disputes = Some(PersistedCacheEntry::Cached(recent_disputes));
	}

	/// Update the earliest session.
	pub fn update_recent_disputes(&mut self, recent_disputes: RecentDisputes) {
		self.cached_recent_disputes = Some(PersistedCacheEntry::Dirty(recent_disputes));
		self.dirty = true;
	}

	/// Get a ref to the recent disputes, if any.
	pub fn get_recent_disputes(&self) -> Option<&RecentDisputes> {
		if let Some(cache_entry) = &self.cached_recent_disputes {
			return Some(cache_entry.value())
		}
		None
	}

	/// Clone the recent disputes, if any.
	pub fn clone_recent_disputes(&self) -> Option<RecentDisputes> {
		if let Some(cache_entry) = &self.cached_recent_disputes {
			return Some(*cache_entry.value())
		}
		None
	}

	/// Initialize vote entries for a candidate.
	pub fn initialize_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
		candidate_votes: CandidateVotes,
	) {
		self.cached_candidate_votes
			.insert((session, *candidate_hash), PersistedCacheEntry::Cached(Some(candidate_votes)));
	}

	/// Update votes for a candidate.
	pub fn update_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
		candidate_votes: Option<CandidateVotes>,
	) {
		if let Some(evicted_entry) = self
			.cached_candidate_votes
			.insert((session, *candidate_hash), PersistedCacheEntry::Dirty(candidate_votes))
		{
			self.evicted_candidate_votes.insert((session, *candidate_hash), evicted_entry);
		}

		self.dirty = true;
	}

	/// Get the candidate votes for the specific session-candidate pair, if present in cache.
	pub fn get_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> Option<&CandidateVotes> {
		if let Some(cached_entry) = self.cached_candidate_votes.get_mut(&(session, *candidate_hash))
		{
			return cached_entry.value().as_ref()
		}
		None
	}

	pub fn is_dirty(&self) -> bool {
		self.dirty
	}

	/// Iterate the cache and convert the dirty entries to a set of write-ops to be written to the inner backend.
	/// TODO: deduplicate the code below.
	pub fn into_write_ops<'a>(&'a mut self) -> impl Iterator<Item = BackendWriteOp<'a>> {
		let earliest_session_ops = self
			.cached_earliest_session
			.as_ref()
			.filter(|entry| entry.is_dirty())
			.map(|session| BackendWriteOp::WriteEarliestSession(*session.value()))
			.into_iter();

		// Update persistence state.
		self.cached_earliest_session = self
			.cached_earliest_session
			.clone()
			.filter(|entry| entry.is_dirty())
			.map(|cache_entry| PersistedCacheEntry::Persisted(cache_entry.into_inner()));

		let recent_dispute_ops = self
			.cached_recent_disputes
			.as_ref()
			.filter(|disputes| disputes.is_dirty())
			.map(|disputes| BackendWriteOp::WriteRecentDisputes(disputes.value()))
			.into_iter();

		// Update persistence state.
		self.cached_recent_disputes = self
			.cached_recent_disputes
			.clone()
			.filter(|entry| entry.is_dirty())
			.map(|cache_entry| PersistedCacheEntry::Persisted(cache_entry.into_inner()));

		let candidate_vote_ops = self
			.cached_candidate_votes
			.clone()
			.iter()
			.filter(|((_, _), votes)| votes.is_dirty())
			.map(|((session, candidate), votes)| match votes.value() {
				Some(votes) => BackendWriteOp::WriteCandidateVotes(*session, *candidate, votes),
				None => BackendWriteOp::DeleteCandidateVotes(*session, *candidate),
			});

		// Scan all `Dirty` cache entries and create updated entries to insert below.
		let updated_candidate_votes = self
			.cached_candidate_votes
			.clone()
			.into_iter()
			.filter(|((_, _), votes)| votes.is_dirty())
			.map(|((session, candidate), votes)| {
				((session, candidate), PersistedCacheEntry::Persisted(votes.into_inner()))
			});

		// Can't figure out how to mutate the enum in place to change state to persisted.
		// Updating all dirty entries here.
		for ((session, candidate), votes) in updated_candidate_votes {
			self.cached_candidate_votes.insert((session, candidate), votes);
		}

		let evicted_candidate_vote_ops = self
			.cached_candidate_votes
			.iter()
			.filter(|((_, _), votes)| votes.is_dirty())
			.map(|((session, candidate), votes)| match votes.value() {
				Some(votes) => BackendWriteOp::WriteCandidateVotes(*session, *candidate, votes),
				None => BackendWriteOp::DeleteCandidateVotes(*session, *candidate),
			});

		self.evicted_candidate_votes.clear();
		self.dirty = false;

		evicted_candidate_vote_ops
			.chain(earliest_session_ops)
			.chain(recent_dispute_ops)
			.chain(candidate_vote_ops)
	}
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	pub fn new(
		backend: &'a B,
		metrics: Metrics,
		cache: &'a mut OverlayCache,
	) -> SubsystemResult<Self> {
		// Populate cache.
		if let Some(session) = backend.load_earliest_session()? {
			let _read_timer = metrics.time_db_read_operation("earliest_session");
			cache.initialize_earliest_sesion(session);
		}

		if let Some(disputes) = backend.load_recent_disputes()? {
			let _read_timer = metrics.time_db_read_operation("recent_disputes");
			cache.initialize_recent_disputes(disputes);
		}

		Ok(Self { inner: backend, cache, metrics })
	}

	/// Returns true if the are no write operations to perform.
	pub fn is_empty(&self) -> bool {
		// self.cache.earliest_session.is_none() &&
		// 	self.cache.recent_disputes.is_none() &&
		// 	self.cache.candidate_votes.is_empty()
		false
	}

	/// Load the earliest session, if any.
	pub fn load_earliest_session(&self) -> SubsystemResult<Option<SessionIndex>> {
		if let Some(val) = self.cache.get_earliest_session() {
			return Ok(Some(*val))
		}

		let _read_timer = self.metrics.time_db_read_operation("earliest_session");
		self.inner.load_earliest_session()
	}

	/// Load the recent disputes, if any.
	pub fn load_recent_disputes(&self) -> SubsystemResult<Option<&RecentDisputes>> {
		if let Some(disputes) = self.cache.get_recent_disputes() {
			return Ok(Some(disputes))
		}

		let read_timer = self.metrics.time_db_read_operation("recent_disputes");
		let disputes = self.inner.load_recent_disputes()?;
		drop(read_timer);

		if let Some(disputes) = disputes {
			self.cache.update_recent_disputes(disputes);
			Ok(self.cache.get_recent_disputes())
		} else {
			Ok(None)
		}
	}

	/// Load the recent disputes, if any.
	pub fn clone_recent_disputes(&self) -> SubsystemResult<Option<RecentDisputes>> {
		if let Some(disputes) = self.cache.get_recent_disputes() {
			return Ok(Some(*disputes))
		}

		let read_timer = self.metrics.time_db_read_operation("recent_disputes");
		let disputes = self.inner.load_recent_disputes()?;
		drop(read_timer);

		if let Some(disputes) = disputes {
			self.cache.update_recent_disputes(disputes.clone());
			Ok(Some(disputes))
		} else {
			Ok(None)
		}
	}

	/// Get a ref to the candidate votes for the specific session-candidate pair, if any.
	pub fn load_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<&CandidateVotes>> {
		if let Some(candidate_votes) = self.cache.get_candidate_votes(session, candidate_hash) {
			return Ok(Some(candidate_votes))
		}

		let read_timer = self.metrics.time_db_read_operation("candidate_votes");
		let votes = self.inner.load_candidate_votes(session, candidate_hash)?;
		drop(read_timer);

		if let Some(votes) = votes {
			self.cache.update_candidate_votes(session, candidate_hash, Some(votes));
			Ok(self.cache.get_candidate_votes(session, candidate_hash))
		} else {
			Ok(None)
		}
	}

	/// Clone the candidate votes for the specific session-candidate pair, if any.
	pub fn clone_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateVotes>> {
		if let Some(candidate_votes) = self.cache.get_candidate_votes(session, candidate_hash) {
			return Ok(Some(*candidate_votes))
		}

		let read_timer = self.metrics.time_db_read_operation("candidate_votes");
		let votes = self.inner.load_candidate_votes(session, candidate_hash)?;
		drop(read_timer);

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
	pub fn write_earliest_session(&self, session: SessionIndex) {
		self.cache.update_earliest_sesion(session);
	}

	/// Prepare a write to the recent disputes stored in the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub fn write_recent_disputes(&self, recent_disputes: RecentDisputes) {
		self.cache.update_recent_disputes(recent_disputes);
	}

	/// Prepare a write of the candidate votes under the indicated candidate.
	///
	/// Later calls to this function for the same candidate will override earlier ones.
	pub fn write_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: CandidateHash,
		votes: CandidateVotes,
	) {
		self.cache.update_candidate_votes(session, &candidate_hash, Some(votes));
	}

	/// Prepare a deletion of the candidate votes under the indicated candidate.
	///
	/// Later calls to this function for the same candidate will override earlier ones.
	pub fn delete_candidate_votes(&self, session: SessionIndex, candidate_hash: CandidateHash) {
		// This will dirty the cache entry, not actually remove it.
		self.cache.update_candidate_votes(session, &candidate_hash, None);
	}
}
