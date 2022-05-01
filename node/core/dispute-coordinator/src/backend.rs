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
use std::{collections::HashMap, time::Duration};

use super::db::v1::{CandidateVotes, RecentDisputes};
use crate::{error::FatalResult, metrics::Metrics};

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
	cache: &'a mut OverlayCache,
	// Reports read operations.
	metrics: Metrics,
}

/// Cache entry state definitions.
#[derive(Clone, Debug)]
pub enum PersistedCacheEntryState {
	/// The entry is in memory and is unmodified. This is the initial state.
	Cached,
	/// The entry is in memory and was modified (needs to be persisted later).
	Dirty,
	/// The entry is cached and has been persisted in the DB.
	Persisted,
}

/// A wrapper for changes that will be eventually saved in the DB.

#[derive(Clone, Debug)]
struct PersistedCacheEntry<T> {
	inner: T,
	state: PersistedCacheEntryState,
}

impl From<PersistedCacheEntry<RecentDisputes>> for RecentDisputes {
	fn from(cache_entry: PersistedCacheEntry<RecentDisputes>) -> RecentDisputes {
		cache_entry.inner
	}
}

impl From<PersistedCacheEntry<Option<CandidateVotes>>> for Option<CandidateVotes> {
	fn from(cache_entry: PersistedCacheEntry<Option<CandidateVotes>>) -> Option<CandidateVotes> {
		cache_entry.inner
	}
}

impl<T> PersistedCacheEntry<T> {
	pub fn new(inner: T) -> Self {
		Self { inner, state: PersistedCacheEntryState::Cached }
	}

	pub fn is_dirty(&self) -> bool {
		match self.state {
			PersistedCacheEntryState::Dirty => true,
			_ => false,
		}
	}

	pub fn persist(&mut self) {
		self.state = PersistedCacheEntryState::Persisted;
	}

	pub fn dirty(&mut self) {
		self.state = PersistedCacheEntryState::Dirty;
	}

	/// Convenience method to get a ref to inner value. Avoids matching in the calling code.
	pub fn value(&self) -> &T {
		&self.inner
	}
}

/// The LRU cache size for candidate votes.
const CACHE_SIZE: usize = 1024;

/// The write back interval in seconds
#[cfg(not(test))]
pub const WRITE_BACK_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(test)]
pub const WRITE_BACK_INTERVAL: Duration = Duration::from_secs(0);

/// A cache caching reads (LRU) and batches writes (API) as `BackendWriteOp` that can
/// be fed to a `Backend`.
///
/// How it works:
/// - Candidate votes, session and disputes are kept in memory in the LRU cache and their
/// state is tracked (Cached, Dirty, Persisted).
/// - Once the cache is full (`CACHE_SIZE`), we start moving evicting entries into
/// `evicted_candidate_votes`.
/// - If entries are modified via `update_*`, then these will be marked `Dirty`
///
/// How do I get the `BackendWriteOps`?
/// - `into_write_ops()` scans the LRU cache and the evicted hashmap to construct
/// a list of `BackendWriteOps`.
/// - This method is supposed to be called when we want to write in-memory changes
/// to the DB. It will consume the cache in the process, but it will return a new one
/// with the same LRU cached values.

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
		self.cached_earliest_session = Some(PersistedCacheEntry::new(earliest_session));
	}

	/// Update the earliest session.
	pub fn update_earliest_sesion(&mut self, earliest_session: SessionIndex) {
		let mut entry = PersistedCacheEntry::new(earliest_session);
		entry.dirty();
		self.cached_earliest_session = Some(entry);
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
		self.cached_recent_disputes = Some(PersistedCacheEntry::new(recent_disputes));
	}

	/// Update the earliest session.
	pub fn update_recent_disputes(&mut self, recent_disputes: RecentDisputes) {
		let mut entry = PersistedCacheEntry::new(recent_disputes);
		entry.dirty();
		self.cached_recent_disputes = Some(entry);
		self.dirty = true;
	}

	/// Clone the recent disputes, if any.
	pub fn load_recent_disputes(&self) -> Option<RecentDisputes> {
		if let Some(cache_entry) = self.cached_recent_disputes.clone() {
			return Some(cache_entry.into())
		}
		None
	}

	/// Update votes for a candidate.
	pub fn update_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
		candidate_votes: Option<CandidateVotes>,
	) {
		let mut candidate_votes = PersistedCacheEntry::new(candidate_votes);
		candidate_votes.dirty();

		if self.cached_candidate_votes.len() > CACHE_SIZE {
			// We need to move an entry to evicted entry list.
			if let Some(evicted_entry) = self.cached_candidate_votes.remove_lru() {
				if evicted_entry.1.is_dirty() {
					// Queue the entry for writing to DB.
					self.evicted_candidate_votes.insert(evicted_entry.0, evicted_entry.1);
				}
			}
		}

		self.cached_candidate_votes.insert((session, *candidate_hash), candidate_votes);
		self.dirty = true;
	}

	/// Get the candidate votes for the specific session-candidate pair, if present in cache.
	/// The extra option wrapper in the result is to implement the `None` -> `Delete` semantics
	/// when convering to `BackendWriteOp`.
	pub fn load_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> Option<Option<CandidateVotes>> {
		let mut entry = self.cached_candidate_votes.remove(&(session, *candidate_hash));

		if entry.is_none() {
			entry = self.evicted_candidate_votes.remove(&(session, *candidate_hash));
			if entry.is_none() {
				return None
			}
		}

		let entry = entry.expect("tested above; qed");

		// Bring the entry back to the LRU cache if we pick up an evicted one.
		self.update_candidate_votes(session, candidate_hash, entry.clone().into());

		Some(entry.into())
	}

	pub fn is_dirty(&self) -> bool {
		self.dirty
	}

	/// Consumes `self` and returns a brand new `Self` and set of backend write ops if the `OverlayCache` is dirty.
	pub fn into_write_ops(
		mut self,
	) -> (OverlayCache, Option<impl Iterator<Item = BackendWriteOp>>) {
		// Bail out early if no changes recorded.
		if !self.is_dirty() {
			return (self, None)
		}

		let earliest_session_ops = self
			.cached_earliest_session
			.clone()
			.filter(|entry| entry.is_dirty())
			.map(|entry| BackendWriteOp::WriteEarliestSession(*entry.value()))
			.into_iter();

		let cached_earliest_session = self.cached_earliest_session.map(|mut entry| {
			if entry.is_dirty() {
				entry.persist()
			}
			entry
		});

		let mut recent_dispute_ops = Vec::new();

		let cached_recent_disputes =
			if let Some(mut recent_disputes) = self.cached_recent_disputes.take() {
				if recent_disputes.is_dirty() {
					recent_disputes.persist();
					recent_dispute_ops
						.push(BackendWriteOp::WriteRecentDisputes(recent_disputes.value().clone()));
				}
				Some(recent_disputes)
			} else {
				None
			};

		let mut candidate_vote_ops = Vec::new();
		let mut cached_candidate_votes = LruCache::new(CACHE_SIZE);

		self.cached_candidate_votes
			.into_iter()
			.for_each(|((session, candidate), mut votes)| {
				if votes.is_dirty() {
					votes.persist();
					candidate_vote_ops.push(match votes.value() {
						Some(votes) =>
							BackendWriteOp::WriteCandidateVotes(session, candidate, votes.clone()),
						None => BackendWriteOp::DeleteCandidateVotes(session, candidate),
					});
				}
				cached_candidate_votes.insert((session, candidate), votes);
			});

		let evicted_candidate_vote_ops = self
			.evicted_candidate_votes
			.into_iter()
			// Ensure we are only writing the dirty ones, the others can be dropped.
			.filter(|((_, _), votes)| votes.is_dirty())
			.map(|((session, candidate), votes)| {
				let inner = votes.into();
				match inner {
					Some(votes) => BackendWriteOp::WriteCandidateVotes(session, candidate, votes),
					None => BackendWriteOp::DeleteCandidateVotes(session, candidate),
				}
			});

		let ops = evicted_candidate_vote_ops
			.chain(earliest_session_ops)
			.chain(recent_dispute_ops.into_iter())
			.chain(candidate_vote_ops);

		(
			OverlayCache {
				cached_earliest_session,
				cached_recent_disputes,
				cached_candidate_votes,
				evicted_candidate_votes: HashMap::new(),
				dirty: false,
			},
			Some(ops),
		)
	}
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	pub fn new(
		backend: &'a B,
		metrics: Metrics,
		cache: &'a mut OverlayCache,
	) -> SubsystemResult<Self> {
		// Initialize some caches.
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

	/// Load the earliest session, if any.
	pub fn load_earliest_session(&self) -> SubsystemResult<Option<SessionIndex>> {
		if let Some(val) = self.cache.get_earliest_session() {
			return Ok(Some(*val))
		}

		let _read_timer = self.metrics.time_db_read_operation("earliest_session");
		self.inner.load_earliest_session()
	}

	/// Load the recent disputes, if any.
	pub fn load_recent_disputes(&mut self) -> SubsystemResult<Option<RecentDisputes>> {
		if let Some(disputes) = self.cache.load_recent_disputes() {
			return Ok(Some(disputes))
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

	/// Clone the candidate votes for the specific session-candidate pair, if any.
	pub fn load_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateVotes>> {
		if let Some(candidate_votes) = self.cache.load_candidate_votes(session, candidate_hash) {
			return Ok(candidate_votes)
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
