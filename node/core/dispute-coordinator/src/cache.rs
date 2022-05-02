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

//! An LRU cache implementation for [`OverlayedBackend`].

use polkadot_primitives::v2::{CandidateHash, SessionIndex};

use lru_cache::LruCache;
use std::{collections::HashMap, time::Duration};

use super::db::v1::{CandidateVotes, RecentDisputes};
use crate::backend::BackendWriteOp;

/// An arbitrary LRU cache size for candidate votes.
const CACHE_SIZE: usize = 1024;

/// The write back interval in seconds
#[cfg(not(test))]
pub const WRITE_BACK_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(test)]
pub const WRITE_BACK_INTERVAL: Duration = Duration::from_secs(0);

/// A read-only cache (LRU) which records changes and outputs them as `BackendWriteOp` which can
/// be fed to a `Backend` for persisting to the DB. It allows the subsystem to avoid
/// calling into the database to retrieve or write data.
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

	// All entries here need to be `Dirty`.
	evicted_candidate_votes:
		HashMap<(SessionIndex, CandidateHash), PersistedCacheEntry<Option<CandidateVotes>>>,
	dirty: bool,
	capacity: usize,
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

impl OverlayCache {
	pub fn new() -> Self {
		Self::new_with_capacity(CACHE_SIZE)
	}

	pub fn new_with_capacity(capacity: usize) -> Self {
		OverlayCache {
			cached_earliest_session: None,
			cached_recent_disputes: None,
			cached_candidate_votes: LruCache::new(capacity),
			evicted_candidate_votes: HashMap::new(),
			dirty: false,
			capacity,
		}
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

		// Check if this is an insert and exceeds capacity.
		if !self.cached_candidate_votes.contains_key(&(session, *candidate_hash)) {
			if self.cached_candidate_votes.len() >= self.capacity {
				// We need to move an entry to evicted entry list.
				if let Some(evicted_entry) = self.cached_candidate_votes.remove_lru() {
					if evicted_entry.1.is_dirty() {
						// Queue the entry for writing to DB.
						self.evicted_candidate_votes.insert(evicted_entry.0, evicted_entry.1);
					}
				}
			}
		} else {
			println!("{:?} is known key", (session, *candidate_hash));
		}

		self.cached_candidate_votes.insert((session, *candidate_hash), candidate_votes);
		self.dirty = true;
	}

	/// Get the candidate votes for the specific session-candidate pair, if present in cache.
	/// The extra option wrapper in the result is to implement the `None` -> `Delete` semantics
	/// when convering to `BackendWriteOp`.
	///
	/// TODO (asap) - refactor to use an enum instead of `Option<Option<CandidateVotes>>`.
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
		let mut cached_candidate_votes = LruCache::new(self.capacity);

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
				capacity: self.capacity,
			},
			Some(ops),
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::status::DisputeStatus;
	use ::test_helpers::{dummy_candidate_receipt, dummy_hash};
	use polkadot_primitives::v2::{Hash, Id as ParaId};

	#[test]
	fn overlay_cache_into_write_ops() {
		let mut overlay_cache = OverlayCache::new_with_capacity(1);

		assert!(!overlay_cache.is_dirty());
		for session in 0..100 {
			overlay_cache.update_earliest_sesion(session);
		}
		overlay_cache.update_earliest_sesion(1234);

		assert_eq!(*overlay_cache.get_earliest_session().unwrap(), 1234);

		let (mut overlay_cache, write_ops) = overlay_cache.into_write_ops();
		let mut write_ops = write_ops.unwrap();
		assert_eq!(write_ops.next().unwrap(), BackendWriteOp::WriteEarliestSession(1234));
		assert!(write_ops.next().is_none());

		let disputes =
			vec![((0, CandidateHash(Hash::repeat_byte(10))), DisputeStatus::Confirmed); 100]
				.into_iter()
				.collect::<RecentDisputes>();

		overlay_cache.update_recent_disputes(disputes.clone());

		let (mut overlay_cache, write_ops) = overlay_cache.into_write_ops();
		let mut write_ops = write_ops.unwrap();
		assert_eq!(
			write_ops.next().unwrap(),
			BackendWriteOp::WriteRecentDisputes(disputes.clone())
		);
		assert!(write_ops.next().is_none());

		assert_eq!(overlay_cache.load_recent_disputes().unwrap(), disputes);

		let votes = CandidateVotes {
			candidate_receipt: {
				let mut receipt = dummy_candidate_receipt(dummy_hash());
				receipt.descriptor.para_id = ParaId::new(2000);

				receipt
			},
			valid: Vec::new(),
			invalid: Vec::new(),
		};

		overlay_cache.update_candidate_votes(
			1,
			&CandidateHash(Hash::repeat_byte(1)),
			Some(votes.clone()),
		);

		let (mut overlay_cache, write_ops) = overlay_cache.into_write_ops();
		let mut write_ops = write_ops.unwrap();
		assert_eq!(
			write_ops.next().unwrap(),
			BackendWriteOp::WriteCandidateVotes(
				1,
				CandidateHash(Hash::repeat_byte(1)),
				votes.clone()
			)
		);
		assert!(write_ops.next().is_none());
		assert_eq!(
			overlay_cache
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap()
				.unwrap(),
			votes
		);

		// Delete the votes
		overlay_cache.update_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)), None);

		// Check if we have the key and value set to none, as we expect to delete the entry when writing the DB.
		assert_eq!(
			overlay_cache.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1))),
			Some(None)
		);

		let (mut overlay_cache, write_ops) = overlay_cache.into_write_ops();
		let mut write_ops = write_ops.unwrap();

		assert_eq!(
			write_ops.next().unwrap(),
			BackendWriteOp::DeleteCandidateVotes(1, CandidateHash(Hash::repeat_byte(1)),)
		);
		assert!(write_ops.next().is_none());
		// Check if it is still in cache.
		assert_eq!(
			overlay_cache
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap(),
			None
		);
	}

	#[test]
	fn overlay_cache_rw_and_eviction() {
		let mut overlay_cache = OverlayCache::new_with_capacity(1);

		assert!(!overlay_cache.is_dirty());

		overlay_cache.update_earliest_sesion(0);
		assert!(overlay_cache.is_dirty());

		overlay_cache.update_earliest_sesion(1);
		overlay_cache.update_earliest_sesion(2);
		overlay_cache.update_earliest_sesion(3);
		overlay_cache.update_earliest_sesion(4);
		overlay_cache.update_earliest_sesion(5);
		overlay_cache.update_earliest_sesion(1);

		overlay_cache.update_recent_disputes(
			vec![((0, CandidateHash(Hash::repeat_byte(0))), DisputeStatus::Active)]
				.into_iter()
				.collect(),
		);

		overlay_cache.update_recent_disputes(
			vec![((1, CandidateHash(Hash::repeat_byte(1))), DisputeStatus::Active)]
				.into_iter()
				.collect(),
		);

		overlay_cache.update_candidate_votes(
			1,
			&CandidateHash(Hash::repeat_byte(1)),
			Some(CandidateVotes {
				candidate_receipt: dummy_candidate_receipt(dummy_hash()),
				valid: Vec::new(),
				invalid: Vec::new(),
			}),
		);

		assert_eq!(overlay_cache.evicted_candidate_votes.len(), 0);

		// Updates the same entry as above, shouldn't cause eviction.
		overlay_cache.update_candidate_votes(
			1,
			&CandidateHash(Hash::repeat_byte(1)),
			Some(CandidateVotes {
				candidate_receipt: {
					let mut receipt = dummy_candidate_receipt(dummy_hash());
					receipt.descriptor.para_id = 5.into();

					receipt
				},
				valid: Vec::new(),
				invalid: Vec::new(),
			}),
		);

		overlay_cache.update_candidate_votes(
			2,
			&CandidateHash(Hash::repeat_byte(2)),
			Some(CandidateVotes {
				candidate_receipt: dummy_candidate_receipt(dummy_hash()),
				valid: Vec::new(),
				invalid: Vec::new(),
			}),
		);

		// The first candidate element is evicted.
		assert_eq!(overlay_cache.evicted_candidate_votes.len(), 1);

		assert_eq!(*overlay_cache.get_earliest_session().unwrap(), 1);

		assert_eq!(
			overlay_cache.load_recent_disputes().unwrap(),
			vec![((1, CandidateHash(Hash::repeat_byte(1))), DisputeStatus::Active),]
				.into_iter()
				.collect()
		);

		assert_eq!(
			overlay_cache
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap()
				.unwrap()
				.candidate_receipt
				.descriptor
				.para_id,
			ParaId::from(5),
		);

		let (mut overlay_cache, _) = overlay_cache.into_write_ops();

		// Evicted items are purged when consuming self.
		assert_eq!(overlay_cache.evicted_candidate_votes.len(), 0);

		// Test that subsequent writes were recorded.
		assert_eq!(*overlay_cache.get_earliest_session().unwrap(), 1);

		// This should fail, as the entry was evicted.
		assert_eq!(
			overlay_cache.load_recent_disputes().unwrap(),
			vec![((1, CandidateHash(Hash::repeat_byte(1))), DisputeStatus::Active),]
				.into_iter()
				.collect()
		);

		assert_eq!(
			overlay_cache
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap()
				.unwrap()
				.candidate_receipt
				.descriptor
				.para_id,
			ParaId::from(5),
		);
	}
}
