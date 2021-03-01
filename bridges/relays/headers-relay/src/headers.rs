// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Headers queue - the intermediate buffer that is filled when headers are read
//! from the source chain. Headers are removed from the queue once they become
//! known to the target chain. Inside, there are several sub-queues, where headers
//! may stay until source/target chain state isn't updated. When a header reaches the
//! `ready` sub-queue, it may be submitted to the target chain.

use crate::sync_types::{HeaderIdOf, HeaderStatus, HeadersSyncPipeline, QueuedHeader, SourceHeader};

use linked_hash_map::LinkedHashMap;
use num_traits::{One, Zero};
use relay_utils::HeaderId;
use std::{
	collections::{btree_map::Entry as BTreeMapEntry, hash_map::Entry as HashMapEntry, BTreeMap, HashMap, HashSet},
	time::{Duration, Instant},
};

type HeadersQueue<P> =
	BTreeMap<<P as HeadersSyncPipeline>::Number, HashMap<<P as HeadersSyncPipeline>::Hash, QueuedHeader<P>>>;
type SyncedChildren<P> =
	BTreeMap<<P as HeadersSyncPipeline>::Number, HashMap<<P as HeadersSyncPipeline>::Hash, HashSet<HeaderIdOf<P>>>>;
type KnownHeaders<P> =
	BTreeMap<<P as HeadersSyncPipeline>::Number, HashMap<<P as HeadersSyncPipeline>::Hash, HeaderStatus>>;

/// We're trying to fetch completion data for single header at this interval.
const RETRY_FETCH_COMPLETION_INTERVAL: Duration = Duration::from_secs(20);

/// Headers queue.
#[derive(Debug)]
pub struct QueuedHeaders<P: HeadersSyncPipeline> {
	/// Headers that are received from source node, but we (native sync code) have
	/// never seen their parents. So we need to check if we can/should submit this header.
	maybe_orphan: HeadersQueue<P>,
	/// Headers that are received from source node, and we (native sync code) have
	/// checked that Substrate runtime doesn't know their parents. So we need to submit parents
	/// first.
	orphan: HeadersQueue<P>,
	/// Headers that are ready to be submitted to target node, but we need to check
	/// whether submission requires extra data to be provided.
	maybe_extra: HeadersQueue<P>,
	/// Headers that are ready to be submitted to target node, but we need to retrieve
	/// extra data first.
	extra: HeadersQueue<P>,
	/// Headers that are ready to be submitted to target node.
	ready: HeadersQueue<P>,
	/// Headers that are ready to be submitted to target node, but their ancestor is incomplete.
	/// Thus we're waiting for these ancestors to be completed first.
	/// Note that the incomplete header itself is synced and it isn't in this queue.
	incomplete: HeadersQueue<P>,
	/// Headers that are (we believe) currently submitted to target node by our,
	/// not-yet mined transactions.
	submitted: HeadersQueue<P>,
	/// Synced headers childrens. We need it to support case when header is synced, but some of
	/// its parents are incomplete.
	synced_children: SyncedChildren<P>,
	/// Pointers to all headers that we ever seen and we believe we can touch in the future.
	known_headers: KnownHeaders<P>,
	/// Headers that are waiting for completion data from source node. Mapped (and auto-sorted
	/// by) to the last fetch time.
	incomplete_headers: LinkedHashMap<HeaderIdOf<P>, Option<Instant>>,
	/// Headers that are waiting to be completed at target node. Auto-sorted by insertion time.
	completion_data: LinkedHashMap<HeaderIdOf<P>, P::Completion>,
	/// Best synced block number.
	best_synced_number: P::Number,
	/// Pruned blocks border. We do not store or accept any blocks with number less than
	/// this number.
	prune_border: P::Number,
}

/// Header completion data.
#[derive(Debug)]
struct HeaderCompletion<Completion> {
	/// Last time when we tried to upload completion data to target node, if ever.
	pub last_upload_time: Option<Instant>,
	/// Completion data.
	pub completion: Completion,
}

impl<P: HeadersSyncPipeline> Default for QueuedHeaders<P> {
	fn default() -> Self {
		QueuedHeaders {
			maybe_orphan: HeadersQueue::new(),
			orphan: HeadersQueue::new(),
			maybe_extra: HeadersQueue::new(),
			extra: HeadersQueue::new(),
			ready: HeadersQueue::new(),
			incomplete: HeadersQueue::new(),
			submitted: HeadersQueue::new(),
			synced_children: SyncedChildren::<P>::new(),
			known_headers: KnownHeaders::<P>::new(),
			incomplete_headers: LinkedHashMap::new(),
			completion_data: LinkedHashMap::new(),
			best_synced_number: Zero::zero(),
			prune_border: Zero::zero(),
		}
	}
}

impl<P: HeadersSyncPipeline> QueuedHeaders<P> {
	/// Returns prune border.
	#[cfg(test)]
	pub fn prune_border(&self) -> P::Number {
		self.prune_border
	}

	/// Returns number of headers that are currently in given queue.
	pub fn headers_in_status(&self, status: HeaderStatus) -> usize {
		match status {
			HeaderStatus::Unknown | HeaderStatus::Synced => 0,
			HeaderStatus::MaybeOrphan => self
				.maybe_orphan
				.values()
				.fold(0, |total, headers| total + headers.len()),
			HeaderStatus::Orphan => self.orphan.values().fold(0, |total, headers| total + headers.len()),
			HeaderStatus::MaybeExtra => self
				.maybe_extra
				.values()
				.fold(0, |total, headers| total + headers.len()),
			HeaderStatus::Extra => self.extra.values().fold(0, |total, headers| total + headers.len()),
			HeaderStatus::Ready => self.ready.values().fold(0, |total, headers| total + headers.len()),
			HeaderStatus::Incomplete => self.incomplete.values().fold(0, |total, headers| total + headers.len()),
			HeaderStatus::Submitted => self.submitted.values().fold(0, |total, headers| total + headers.len()),
		}
	}

	/// Returns number of headers that are currently in the queue.
	pub fn total_headers(&self) -> usize {
		self.maybe_orphan
			.values()
			.fold(0, |total, headers| total + headers.len())
			+ self.orphan.values().fold(0, |total, headers| total + headers.len())
			+ self
				.maybe_extra
				.values()
				.fold(0, |total, headers| total + headers.len())
			+ self.extra.values().fold(0, |total, headers| total + headers.len())
			+ self.ready.values().fold(0, |total, headers| total + headers.len())
			+ self.incomplete.values().fold(0, |total, headers| total + headers.len())
	}

	/// Returns number of best block in the queue.
	pub fn best_queued_number(&self) -> P::Number {
		std::cmp::max(
			self.maybe_orphan.keys().next_back().cloned().unwrap_or_else(Zero::zero),
			std::cmp::max(
				self.orphan.keys().next_back().cloned().unwrap_or_else(Zero::zero),
				std::cmp::max(
					self.maybe_extra.keys().next_back().cloned().unwrap_or_else(Zero::zero),
					std::cmp::max(
						self.extra.keys().next_back().cloned().unwrap_or_else(Zero::zero),
						std::cmp::max(
							self.ready.keys().next_back().cloned().unwrap_or_else(Zero::zero),
							std::cmp::max(
								self.incomplete.keys().next_back().cloned().unwrap_or_else(Zero::zero),
								self.submitted.keys().next_back().cloned().unwrap_or_else(Zero::zero),
							),
						),
					),
				),
			),
		)
	}

	/// Returns number of best synced block we have ever seen. It is either less
	/// than `best_queued_number()`, or points to last synced block if queue is empty.
	pub fn best_synced_number(&self) -> P::Number {
		self.best_synced_number
	}

	/// Returns synchronization status of the header.
	pub fn status(&self, id: &HeaderIdOf<P>) -> HeaderStatus {
		self.known_headers
			.get(&id.0)
			.and_then(|x| x.get(&id.1))
			.cloned()
			.unwrap_or(HeaderStatus::Unknown)
	}

	/// Get oldest header from given queue.
	pub fn header(&self, status: HeaderStatus) -> Option<&QueuedHeader<P>> {
		match status {
			HeaderStatus::Unknown | HeaderStatus::Synced => None,
			HeaderStatus::MaybeOrphan => oldest_header(&self.maybe_orphan),
			HeaderStatus::Orphan => oldest_header(&self.orphan),
			HeaderStatus::MaybeExtra => oldest_header(&self.maybe_extra),
			HeaderStatus::Extra => oldest_header(&self.extra),
			HeaderStatus::Ready => oldest_header(&self.ready),
			HeaderStatus::Incomplete => oldest_header(&self.incomplete),
			HeaderStatus::Submitted => oldest_header(&self.submitted),
		}
	}

	/// Get oldest headers from given queue until functor will return false.
	pub fn headers(
		&self,
		status: HeaderStatus,
		f: impl FnMut(&QueuedHeader<P>) -> bool,
	) -> Option<Vec<&QueuedHeader<P>>> {
		match status {
			HeaderStatus::Unknown | HeaderStatus::Synced => None,
			HeaderStatus::MaybeOrphan => oldest_headers(&self.maybe_orphan, f),
			HeaderStatus::Orphan => oldest_headers(&self.orphan, f),
			HeaderStatus::MaybeExtra => oldest_headers(&self.maybe_extra, f),
			HeaderStatus::Extra => oldest_headers(&self.extra, f),
			HeaderStatus::Ready => oldest_headers(&self.ready, f),
			HeaderStatus::Incomplete => oldest_headers(&self.incomplete, f),
			HeaderStatus::Submitted => oldest_headers(&self.submitted, f),
		}
	}

	/// Appends new header, received from the source node, to the queue.
	pub fn header_response(&mut self, header: P::Header) {
		let id = header.id();
		let status = self.status(&id);
		if status != HeaderStatus::Unknown {
			log::debug!(
				target: "bridge",
				"Ignoring new {} header: {:?}. Status is {:?}.",
				P::SOURCE_NAME,
				id,
				status,
			);
			return;
		}

		if id.0 < self.prune_border {
			log::debug!(
				target: "bridge",
				"Ignoring ancient new {} header: {:?}.",
				P::SOURCE_NAME,
				id,
			);
			return;
		}

		let parent_id = header.parent_id();
		let parent_status = self.status(&parent_id);
		let header = QueuedHeader::new(header);

		let status = match parent_status {
			HeaderStatus::Unknown | HeaderStatus::MaybeOrphan => {
				insert_header(&mut self.maybe_orphan, id, header);
				HeaderStatus::MaybeOrphan
			}
			HeaderStatus::Orphan => {
				insert_header(&mut self.orphan, id, header);
				HeaderStatus::Orphan
			}
			HeaderStatus::MaybeExtra
			| HeaderStatus::Extra
			| HeaderStatus::Ready
			| HeaderStatus::Incomplete
			| HeaderStatus::Submitted
			| HeaderStatus::Synced => {
				insert_header(&mut self.maybe_extra, id, header);
				HeaderStatus::MaybeExtra
			}
		};

		self.known_headers.entry(id.0).or_default().insert(id.1, status);
		log::debug!(
			target: "bridge",
			"Queueing new {} header: {:?}. Queue: {:?}.",
			P::SOURCE_NAME,
			id,
			status,
		);
	}

	/// Receive best header from the target node.
	pub fn target_best_header_response(&mut self, id: &HeaderIdOf<P>) {
		self.header_synced(id)
	}

	/// Receive target node response for MaybeOrphan request.
	pub fn maybe_orphan_response(&mut self, id: &HeaderIdOf<P>, response: bool) {
		if !response {
			move_header_descendants::<P>(
				&mut [&mut self.maybe_orphan],
				&mut self.orphan,
				&mut self.known_headers,
				HeaderStatus::Orphan,
				&id,
			);
			return;
		}

		move_header_descendants::<P>(
			&mut [&mut self.maybe_orphan, &mut self.orphan],
			&mut self.maybe_extra,
			&mut self.known_headers,
			HeaderStatus::MaybeExtra,
			&id,
		);
	}

	/// Receive target node response for MaybeExtra request.
	pub fn maybe_extra_response(&mut self, id: &HeaderIdOf<P>, response: bool) {
		let (destination_status, destination_queue) = if response {
			(HeaderStatus::Extra, &mut self.extra)
		} else if self.is_parent_incomplete(id) {
			(HeaderStatus::Incomplete, &mut self.incomplete)
		} else {
			(HeaderStatus::Ready, &mut self.ready)
		};

		move_header(
			&mut self.maybe_extra,
			destination_queue,
			&mut self.known_headers,
			destination_status,
			&id,
			|header| header,
		);
	}

	/// Receive extra from source node.
	pub fn extra_response(&mut self, id: &HeaderIdOf<P>, extra: P::Extra) {
		let (destination_status, destination_queue) = if self.is_parent_incomplete(id) {
			(HeaderStatus::Incomplete, &mut self.incomplete)
		} else {
			(HeaderStatus::Ready, &mut self.ready)
		};

		// move header itself from extra to ready queue
		move_header(
			&mut self.extra,
			destination_queue,
			&mut self.known_headers,
			destination_status,
			id,
			|header| header.set_extra(extra),
		);
	}

	/// Receive completion response from source node.
	pub fn completion_response(&mut self, id: &HeaderIdOf<P>, completion: Option<P::Completion>) {
		let completion = match completion {
			Some(completion) => completion,
			None => {
				log::debug!(
					target: "bridge",
					"{} Node is still missing completion data for header: {:?}. Will retry later.",
					P::SOURCE_NAME,
					id,
				);

				return;
			}
		};

		// do not remove from `incomplete_headers` here, because otherwise we'll miss
		// completion 'notification'
		// this could lead to duplicate completion retrieval (if completion transaction isn't mined
		// for too long)
		//
		// instead, we're moving entry to the end of the queue, so that completion data won't be
		// refetched instantly
		if self.incomplete_headers.remove(id).is_some() {
			log::debug!(
				target: "bridge",
				"Received completion data from {} for header: {:?}",
				P::SOURCE_NAME,
				id,
			);

			self.completion_data.insert(*id, completion);
			self.incomplete_headers.insert(*id, Some(Instant::now()));
		}
	}

	/// When header is submitted to target node.
	pub fn headers_submitted(&mut self, ids: Vec<HeaderIdOf<P>>) {
		for id in ids {
			move_header(
				&mut self.ready,
				&mut self.submitted,
				&mut self.known_headers,
				HeaderStatus::Submitted,
				&id,
				|header| header,
			);
		}
	}

	/// When header completion data is sent to target node.
	pub fn header_completed(&mut self, id: &HeaderIdOf<P>) {
		if self.completion_data.remove(id).is_some() {
			log::debug!(
				target: "bridge",
				"Sent completion data to {} for header: {:?}",
				P::TARGET_NAME,
				id,
			);

			// transaction can be dropped by target chain nodes => it would never be mined
			//
			// in current implementation the sync loop would wait for some time && if best
			// **source** header won't change on **target** node, then the sync will be restarted
			// => we'll resubmit the same completion data again (the same is true for submitted
			// headers)
			//
			// the other option would be to track emitted transactions at least on target node,
			// but it won't give us 100% guarantee anyway
			//
			// => we're just dropping completion data just after it has been submitted
		}
	}

	/// Marks given headers incomplete.
	pub fn add_incomplete_headers(&mut self, make_header_incomplete: bool, new_incomplete_headers: Vec<HeaderIdOf<P>>) {
		for new_incomplete_header in new_incomplete_headers {
			if make_header_incomplete {
				self.header_synced(&new_incomplete_header);
			}

			let move_origins = select_synced_children::<P>(&self.synced_children, &new_incomplete_header);
			let move_origins = move_origins.into_iter().chain(std::iter::once(new_incomplete_header));
			for move_origin in move_origins {
				move_header_descendants::<P>(
					&mut [&mut self.ready, &mut self.submitted],
					&mut self.incomplete,
					&mut self.known_headers,
					HeaderStatus::Incomplete,
					&move_origin,
				);
			}

			if make_header_incomplete {
				log::debug!(
					target: "bridge",
					"Scheduling completion data retrieval for header: {:?}",
					new_incomplete_header,
				);

				self.incomplete_headers.insert(new_incomplete_header, None);
			}
		}
	}

	/// When incomplete headers ids are receved from target node.
	pub fn incomplete_headers_response(&mut self, ids: HashSet<HeaderIdOf<P>>) {
		// all new incomplete headers are marked Synced and all their descendants
		// are moved from Ready/Submitted to Incomplete queue
		let new_incomplete_headers = ids
			.iter()
			.filter(|id| !self.incomplete_headers.contains_key(id) && !self.completion_data.contains_key(id))
			.cloned()
			.collect::<Vec<_>>();
		self.add_incomplete_headers(true, new_incomplete_headers);

		// for all headers that were incompleted previously, but now are completed, we move
		// all descendants from incomplete to ready
		let just_completed_headers = self
			.incomplete_headers
			.keys()
			.chain(self.completion_data.keys())
			.filter(|id| !ids.contains(id))
			.cloned()
			.collect::<Vec<_>>();
		for just_completed_header in just_completed_headers {
			// sub2eth rejects H if H.Parent is incomplete
			// sub2sub allows 'syncing' headers like that
			// => let's check if there are some synced children of just completed header
			let move_origins = select_synced_children::<P>(&self.synced_children, &just_completed_header);
			let move_origins = move_origins.into_iter().chain(std::iter::once(just_completed_header));
			for move_origin in move_origins {
				move_header_descendants::<P>(
					&mut [&mut self.incomplete],
					&mut self.ready,
					&mut self.known_headers,
					HeaderStatus::Ready,
					&move_origin,
				);
			}

			log::debug!(
				target: "bridge",
				"Completion data is no longer required for header: {:?}",
				just_completed_header,
			);

			self.incomplete_headers.remove(&just_completed_header);
			self.completion_data.remove(&just_completed_header);
		}
	}

	/// Returns true if given header requires completion data.
	pub fn requires_completion_data(&self, id: &HeaderIdOf<P>) -> bool {
		self.incomplete_headers.contains_key(id)
	}

	/// Returns id of the header for which we want to fetch completion data.
	pub fn incomplete_header(&mut self) -> Option<HeaderIdOf<P>> {
		queued_incomplete_header(&mut self.incomplete_headers, |last_fetch_time| {
			let retry = match *last_fetch_time {
				Some(last_fetch_time) => last_fetch_time.elapsed() > RETRY_FETCH_COMPLETION_INTERVAL,
				None => true,
			};

			if retry {
				*last_fetch_time = Some(Instant::now());
			}

			retry
		})
		.map(|(id, _)| id)
	}

	/// Returns header completion data to upload to target node.
	pub fn header_to_complete(&mut self) -> Option<(HeaderIdOf<P>, &P::Completion)> {
		queued_incomplete_header(&mut self.completion_data, |_| true)
	}

	/// Prune and never accept headers before this block.
	pub fn prune(&mut self, prune_border: P::Number) {
		if prune_border <= self.prune_border {
			return;
		}

		prune_queue(&mut self.maybe_orphan, prune_border);
		prune_queue(&mut self.orphan, prune_border);
		prune_queue(&mut self.maybe_extra, prune_border);
		prune_queue(&mut self.extra, prune_border);
		prune_queue(&mut self.ready, prune_border);
		prune_queue(&mut self.submitted, prune_border);
		prune_queue(&mut self.incomplete, prune_border);
		self.synced_children = self.synced_children.split_off(&prune_border);
		prune_known_headers::<P>(&mut self.known_headers, prune_border);
		self.prune_border = prune_border;
	}

	/// Forgets all ever known headers.
	pub fn clear(&mut self) {
		self.maybe_orphan.clear();
		self.orphan.clear();
		self.maybe_extra.clear();
		self.extra.clear();
		self.ready.clear();
		self.incomplete.clear();
		self.submitted.clear();
		self.synced_children.clear();
		self.known_headers.clear();
		self.best_synced_number = Zero::zero();
		self.prune_border = Zero::zero();
	}

	/// Returns true if parent of this header is either incomplete or waiting for
	/// its own incomplete ancestor to be completed.
	fn is_parent_incomplete(&self, id: &HeaderIdOf<P>) -> bool {
		let status = self.status(id);
		let header = match status {
			HeaderStatus::MaybeOrphan => header(&self.maybe_orphan, id),
			HeaderStatus::Orphan => header(&self.orphan, id),
			HeaderStatus::MaybeExtra => header(&self.maybe_extra, id),
			HeaderStatus::Extra => header(&self.extra, id),
			HeaderStatus::Ready => header(&self.ready, id),
			HeaderStatus::Incomplete => header(&self.incomplete, id),
			HeaderStatus::Submitted => header(&self.submitted, id),
			HeaderStatus::Unknown => return false,
			HeaderStatus::Synced => return false,
		};

		match header {
			Some(header) => {
				let parent_id = header.header().parent_id();
				self.incomplete_headers.contains_key(&parent_id)
					|| self.completion_data.contains_key(&parent_id)
					|| self.status(&parent_id) == HeaderStatus::Incomplete
			}
			None => false,
		}
	}

	/// When we receive new Synced header from target node.
	fn header_synced(&mut self, id: &HeaderIdOf<P>) {
		// update best synced block number
		self.best_synced_number = std::cmp::max(self.best_synced_number, id.0);

		// all ancestors of this header are now synced => let's remove them from
		// queues
		let mut current = *id;
		let mut id_processed = false;
		let mut previous_current = None;
		loop {
			let header = match self.status(&current) {
				HeaderStatus::Unknown => break,
				HeaderStatus::MaybeOrphan => remove_header(&mut self.maybe_orphan, &current),
				HeaderStatus::Orphan => remove_header(&mut self.orphan, &current),
				HeaderStatus::MaybeExtra => remove_header(&mut self.maybe_extra, &current),
				HeaderStatus::Extra => remove_header(&mut self.extra, &current),
				HeaderStatus::Ready => remove_header(&mut self.ready, &current),
				HeaderStatus::Incomplete => remove_header(&mut self.incomplete, &current),
				HeaderStatus::Submitted => remove_header(&mut self.submitted, &current),
				HeaderStatus::Synced => break,
			}
			.expect("header has a given status; given queue has the header; qed");

			// remember ids of all the children of the current header
			let synced_children_entry = self
				.synced_children
				.entry(current.0)
				.or_default()
				.entry(current.1)
				.or_default();
			let all_queues = [
				&self.maybe_orphan,
				&self.orphan,
				&self.maybe_extra,
				&self.extra,
				&self.ready,
				&self.incomplete,
				&self.submitted,
			];
			for queue in &all_queues {
				let children_from_queue = queue
					.get(&(current.0 + One::one()))
					.map(|potential_children| {
						potential_children
							.values()
							.filter(|potential_child| potential_child.header().parent_id() == current)
							.map(|child| child.id())
							.collect::<Vec<_>>()
					})
					.unwrap_or_default();
				synced_children_entry.extend(children_from_queue);
			}
			if let Some(previous_current) = previous_current {
				synced_children_entry.insert(previous_current);
			}

			set_header_status::<P>(&mut self.known_headers, &current, HeaderStatus::Synced);

			previous_current = Some(current);
			current = header.parent_id();
			id_processed = true;
		}

		// remember that the header itself is synced
		// (condition is here to avoid duplicate log messages)
		if !id_processed {
			set_header_status::<P>(&mut self.known_headers, &id, HeaderStatus::Synced);
		}

		// now let's move all descendants from maybe_orphan && orphan queues to
		// maybe_extra queue
		move_header_descendants::<P>(
			&mut [&mut self.maybe_orphan, &mut self.orphan],
			&mut self.maybe_extra,
			&mut self.known_headers,
			HeaderStatus::MaybeExtra,
			id,
		);
	}
}

/// Insert header to the queue.
fn insert_header<P: HeadersSyncPipeline>(queue: &mut HeadersQueue<P>, id: HeaderIdOf<P>, header: QueuedHeader<P>) {
	queue.entry(id.0).or_default().insert(id.1, header);
}

/// Remove header from the queue.
fn remove_header<P: HeadersSyncPipeline>(queue: &mut HeadersQueue<P>, id: &HeaderIdOf<P>) -> Option<QueuedHeader<P>> {
	let mut headers_at = match queue.entry(id.0) {
		BTreeMapEntry::Occupied(headers_at) => headers_at,
		BTreeMapEntry::Vacant(_) => return None,
	};

	let header = headers_at.get_mut().remove(&id.1);
	if headers_at.get().is_empty() {
		headers_at.remove();
	}
	header
}

/// Get header from the queue.
fn header<'a, P: HeadersSyncPipeline>(queue: &'a HeadersQueue<P>, id: &HeaderIdOf<P>) -> Option<&'a QueuedHeader<P>> {
	queue.get(&id.0).and_then(|by_hash| by_hash.get(&id.1))
}

/// Move header from source to destination queue.
///
/// Returns ID of parent header, if header has been moved, or None otherwise.
fn move_header<P: HeadersSyncPipeline>(
	source_queue: &mut HeadersQueue<P>,
	destination_queue: &mut HeadersQueue<P>,
	known_headers: &mut KnownHeaders<P>,
	destination_status: HeaderStatus,
	id: &HeaderIdOf<P>,
	prepare: impl FnOnce(QueuedHeader<P>) -> QueuedHeader<P>,
) -> Option<HeaderIdOf<P>> {
	let header = match remove_header(source_queue, id) {
		Some(header) => prepare(header),
		None => return None,
	};

	let parent_id = header.header().parent_id();
	destination_queue.entry(id.0).or_default().insert(id.1, header);
	set_header_status::<P>(known_headers, id, destination_status);

	Some(parent_id)
}

/// Move all descendant headers from the source to destination queue.
fn move_header_descendants<P: HeadersSyncPipeline>(
	source_queues: &mut [&mut HeadersQueue<P>],
	destination_queue: &mut HeadersQueue<P>,
	known_headers: &mut KnownHeaders<P>,
	destination_status: HeaderStatus,
	id: &HeaderIdOf<P>,
) {
	let mut current_number = id.0 + One::one();
	let mut current_parents = HashSet::new();
	current_parents.insert(id.1);

	while !current_parents.is_empty() {
		let mut next_parents = HashSet::new();
		for source_queue in source_queues.iter_mut() {
			let mut source_entry = match source_queue.entry(current_number) {
				BTreeMapEntry::Occupied(source_entry) => source_entry,
				BTreeMapEntry::Vacant(_) => continue,
			};

			let mut headers_to_move = Vec::new();
			let children_at_number = source_entry.get().keys().cloned().collect::<Vec<_>>();
			for key in children_at_number {
				let entry = match source_entry.get_mut().entry(key) {
					HashMapEntry::Occupied(entry) => entry,
					HashMapEntry::Vacant(_) => unreachable!("iterating existing keys; qed"),
				};

				if current_parents.contains(&entry.get().header().parent_id().1) {
					let header_to_move = entry.remove();
					let header_to_move_id = header_to_move.id();
					headers_to_move.push((header_to_move_id, header_to_move));
					set_header_status::<P>(known_headers, &header_to_move_id, destination_status);
				}
			}

			if source_entry.get().is_empty() {
				source_entry.remove();
			}

			next_parents.extend(headers_to_move.iter().map(|(id, _)| id.1));

			destination_queue
				.entry(current_number)
				.or_default()
				.extend(headers_to_move.into_iter().map(|(id, h)| (id.1, h)))
		}

		current_number = current_number + One::one();
		std::mem::swap(&mut current_parents, &mut next_parents);
	}
}

/// Selects (recursive) all synced children of given header.
fn select_synced_children<P: HeadersSyncPipeline>(
	synced_children: &SyncedChildren<P>,
	id: &HeaderIdOf<P>,
) -> Vec<HeaderIdOf<P>> {
	let mut result = Vec::new();
	let mut current_parents = HashSet::new();
	current_parents.insert(*id);

	while !current_parents.is_empty() {
		let mut next_parents = HashSet::new();
		for current_parent in &current_parents {
			let current_parent_synced_children = synced_children
				.get(&current_parent.0)
				.and_then(|by_number_entry| by_number_entry.get(&current_parent.1));
			if let Some(current_parent_synced_children) = current_parent_synced_children {
				for current_parent_synced_child in current_parent_synced_children {
					result.push(*current_parent_synced_child);
					next_parents.insert(*current_parent_synced_child);
				}
			}
		}

		let _ = std::mem::replace(&mut current_parents, next_parents);
	}

	result
}

/// Return oldest header from the queue.
fn oldest_header<P: HeadersSyncPipeline>(queue: &HeadersQueue<P>) -> Option<&QueuedHeader<P>> {
	queue.values().flat_map(|h| h.values()).next()
}

/// Return oldest headers from the queue until functor will return false.
fn oldest_headers<P: HeadersSyncPipeline>(
	queue: &HeadersQueue<P>,
	mut f: impl FnMut(&QueuedHeader<P>) -> bool,
) -> Option<Vec<&QueuedHeader<P>>> {
	let result = queue
		.values()
		.flat_map(|h| h.values())
		.take_while(|h| f(h))
		.collect::<Vec<_>>();
	if result.is_empty() {
		None
	} else {
		Some(result)
	}
}

/// Forget all headers with number less than given.
fn prune_queue<P: HeadersSyncPipeline>(queue: &mut HeadersQueue<P>, prune_border: P::Number) {
	*queue = queue.split_off(&prune_border);
}

/// Forget all known headers with number less than given.
fn prune_known_headers<P: HeadersSyncPipeline>(known_headers: &mut KnownHeaders<P>, prune_border: P::Number) {
	let new_known_headers = known_headers.split_off(&prune_border);
	for (pruned_number, pruned_headers) in &*known_headers {
		for pruned_hash in pruned_headers.keys() {
			log::debug!(target: "bridge", "Pruning header {:?}.", HeaderId(*pruned_number, *pruned_hash));
		}
	}
	*known_headers = new_known_headers;
}

/// Change header status.
fn set_header_status<P: HeadersSyncPipeline>(
	known_headers: &mut KnownHeaders<P>,
	id: &HeaderIdOf<P>,
	status: HeaderStatus,
) {
	log::debug!(
		target: "bridge",
		"{} header {:?} is now {:?}",
		P::SOURCE_NAME,
		id,
		status,
	);
	*known_headers.entry(id.0).or_default().entry(id.1).or_insert(status) = status;
}

/// Returns queued incomplete header with maximal elapsed time since last update.
fn queued_incomplete_header<Id: Clone + Eq + std::hash::Hash, T>(
	map: &mut LinkedHashMap<Id, T>,
	filter: impl FnMut(&mut T) -> bool,
) -> Option<(Id, &T)> {
	// TODO (#84): headers that have been just appended to the end of the queue would have to wait until
	// all previous headers will be retried

	let retry_old_header = map
		.front()
		.map(|(key, _)| key.clone())
		.and_then(|key| map.get_mut(&key).map(filter))
		.unwrap_or(false);
	if retry_old_header {
		let (header_key, header) = map.pop_front().expect("we have checked that front() exists; qed");
		map.insert(header_key, header);
		return map.back().map(|(id, data)| (id.clone(), data));
	}

	None
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use crate::sync_loop_tests::{TestHash, TestHeader, TestHeaderId, TestHeadersSyncPipeline, TestNumber};
	use crate::sync_types::QueuedHeader;

	pub(crate) fn header(number: TestNumber) -> QueuedHeader<TestHeadersSyncPipeline> {
		QueuedHeader::new(TestHeader {
			number,
			hash: hash(number),
			parent_hash: hash(number - 1),
		})
	}

	pub(crate) fn hash(number: TestNumber) -> TestHash {
		number
	}

	pub(crate) fn id(number: TestNumber) -> TestHeaderId {
		HeaderId(number, hash(number))
	}

	#[test]
	fn total_headers_works() {
		// total headers just sums up number of headers in every queue
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue.maybe_orphan.entry(1).or_default().insert(
			hash(1),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.maybe_orphan.entry(1).or_default().insert(
			hash(2),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.maybe_orphan.entry(2).or_default().insert(
			hash(3),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.orphan.entry(3).or_default().insert(
			hash(4),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.maybe_extra.entry(4).or_default().insert(
			hash(5),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.ready.entry(5).or_default().insert(
			hash(6),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.incomplete.entry(6).or_default().insert(
			hash(7),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.total_headers(), 7);
	}

	#[test]
	fn best_queued_number_works() {
		// initially there are headers in MaybeOrphan queue only
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue.maybe_orphan.entry(1).or_default().insert(
			hash(1),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.maybe_orphan.entry(1).or_default().insert(
			hash(2),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		queue.maybe_orphan.entry(3).or_default().insert(
			hash(3),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.best_queued_number(), 3);
		// and then there's better header in Orphan
		queue.orphan.entry(10).or_default().insert(
			hash(10),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.best_queued_number(), 10);
		// and then there's better header in MaybeExtra
		queue.maybe_extra.entry(20).or_default().insert(
			hash(20),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.best_queued_number(), 20);
		// and then there's better header in Ready
		queue.ready.entry(30).or_default().insert(
			hash(30),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.best_queued_number(), 30);
		// and then there's better header in MaybeOrphan again
		queue.maybe_orphan.entry(40).or_default().insert(
			hash(40),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.best_queued_number(), 40);
		// and then there's some header in Incomplete
		queue.incomplete.entry(50).or_default().insert(
			hash(50),
			QueuedHeader::<TestHeadersSyncPipeline>::new(Default::default()),
		);
		assert_eq!(queue.best_queued_number(), 50);
	}

	#[test]
	fn status_works() {
		// all headers are unknown initially
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		assert_eq!(queue.status(&id(10)), HeaderStatus::Unknown);
		// and status is read from the KnownHeaders
		queue
			.known_headers
			.entry(10)
			.or_default()
			.insert(hash(10), HeaderStatus::Ready);
		assert_eq!(queue.status(&id(10)), HeaderStatus::Ready);
	}

	#[test]
	fn header_works() {
		// initially we have oldest header #10
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue.maybe_orphan.entry(10).or_default().insert(hash(1), header(100));
		assert_eq!(
			queue.header(HeaderStatus::MaybeOrphan).unwrap().header().hash,
			hash(100)
		);
		// inserting #20 changes nothing
		queue.maybe_orphan.entry(20).or_default().insert(hash(1), header(101));
		assert_eq!(
			queue.header(HeaderStatus::MaybeOrphan).unwrap().header().hash,
			hash(100)
		);
		// inserting #5 makes it oldest
		queue.maybe_orphan.entry(5).or_default().insert(hash(1), header(102));
		assert_eq!(
			queue.header(HeaderStatus::MaybeOrphan).unwrap().header().hash,
			hash(102)
		);
	}

	#[test]
	fn header_response_works() {
		// when parent is Synced, we insert to MaybeExtra
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Synced);
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::MaybeExtra);

		// when parent is Ready, we insert to MaybeExtra
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Ready);
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::MaybeExtra);

		// when parent is Receipts, we insert to MaybeExtra
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Extra);
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::MaybeExtra);

		// when parent is MaybeExtra, we insert to MaybeExtra
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeExtra);
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::MaybeExtra);

		// when parent is Orphan, we insert to Orphan
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Orphan);
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::Orphan);

		// when parent is MaybeOrphan, we insert to MaybeOrphan
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeOrphan);
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::MaybeOrphan);

		// when parent is unknown, we insert to MaybeOrphan
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue.header_response(header(101).header().clone());
		assert_eq!(queue.status(&id(101)), HeaderStatus::MaybeOrphan);
	}

	#[test]
	fn ancestors_are_synced_on_substrate_best_header_response() {
		// let's say someone else has submitted transaction to bridge that changes
		// its best block to #100. At this time we have:
		// #100 in MaybeOrphan
		// #99 in Orphan
		// #98 in MaybeExtra
		// #97 in Receipts
		// #96 in Ready
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(100)
			.or_default()
			.insert(hash(100), header(100));
		queue
			.known_headers
			.entry(99)
			.or_default()
			.insert(hash(99), HeaderStatus::Orphan);
		queue.orphan.entry(99).or_default().insert(hash(99), header(99));
		queue
			.known_headers
			.entry(98)
			.or_default()
			.insert(hash(98), HeaderStatus::MaybeExtra);
		queue.maybe_extra.entry(98).or_default().insert(hash(98), header(98));
		queue
			.known_headers
			.entry(97)
			.or_default()
			.insert(hash(97), HeaderStatus::Extra);
		queue.extra.entry(97).or_default().insert(hash(97), header(97));
		queue
			.known_headers
			.entry(96)
			.or_default()
			.insert(hash(96), HeaderStatus::Ready);
		queue.ready.entry(96).or_default().insert(hash(96), header(96));
		queue.target_best_header_response(&id(100));

		// then the #100 and all ancestors of #100 (#96..#99) are treated as synced
		assert!(queue.maybe_orphan.is_empty());
		assert!(queue.orphan.is_empty());
		assert!(queue.maybe_extra.is_empty());
		assert!(queue.extra.is_empty());
		assert!(queue.ready.is_empty());
		assert_eq!(queue.known_headers.len(), 5);
		assert!(queue
			.known_headers
			.values()
			.all(|s| s.values().all(|s| *s == HeaderStatus::Synced)));

		// children of synced headers are stored
		assert_eq!(
			vec![id(97)],
			queue.synced_children[&96][&hash(96)]
				.iter()
				.cloned()
				.collect::<Vec<_>>()
		);
		assert_eq!(
			vec![id(98)],
			queue.synced_children[&97][&hash(97)]
				.iter()
				.cloned()
				.collect::<Vec<_>>()
		);
		assert_eq!(
			vec![id(99)],
			queue.synced_children[&98][&hash(98)]
				.iter()
				.cloned()
				.collect::<Vec<_>>()
		);
		assert_eq!(
			vec![id(100)],
			queue.synced_children[&99][&hash(99)]
				.iter()
				.cloned()
				.collect::<Vec<_>>()
		);
		assert_eq!(0, queue.synced_children[&100][&hash(100)].len());
	}

	#[test]
	fn descendants_are_moved_on_substrate_best_header_response() {
		// let's say someone else has submitted transaction to bridge that changes
		// its best block to #100. At this time we have:
		// #101 in Orphan
		// #102 in MaybeOrphan
		// #103 in Orphan
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Orphan);
		queue.orphan.entry(101).or_default().insert(hash(101), header(101));
		queue
			.known_headers
			.entry(102)
			.or_default()
			.insert(hash(102), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(102)
			.or_default()
			.insert(hash(102), header(102));
		queue
			.known_headers
			.entry(103)
			.or_default()
			.insert(hash(103), HeaderStatus::Orphan);
		queue.orphan.entry(103).or_default().insert(hash(103), header(103));
		queue.target_best_header_response(&id(100));

		// all descendants are moved to MaybeExtra
		assert!(queue.maybe_orphan.is_empty());
		assert!(queue.orphan.is_empty());
		assert_eq!(queue.maybe_extra.len(), 3);
		assert_eq!(queue.known_headers[&101][&hash(101)], HeaderStatus::MaybeExtra);
		assert_eq!(queue.known_headers[&102][&hash(102)], HeaderStatus::MaybeExtra);
		assert_eq!(queue.known_headers[&103][&hash(103)], HeaderStatus::MaybeExtra);
	}

	#[test]
	fn positive_maybe_orphan_response_works() {
		// let's say we have:
		// #100 in MaybeOrphan
		// #101 in Orphan
		// #102 in MaybeOrphan
		// and we have asked for MaybeOrphan status of #100.parent (i.e. #99)
		// and the response is: YES, #99 is known to the Substrate runtime
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(100)
			.or_default()
			.insert(hash(100), header(100));
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Orphan);
		queue.orphan.entry(101).or_default().insert(hash(101), header(101));
		queue
			.known_headers
			.entry(102)
			.or_default()
			.insert(hash(102), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(102)
			.or_default()
			.insert(hash(102), header(102));
		queue.maybe_orphan_response(&id(99), true);

		// then all headers (#100..#103) are moved to the MaybeExtra queue
		assert!(queue.orphan.is_empty());
		assert!(queue.maybe_orphan.is_empty());
		assert_eq!(queue.maybe_extra.len(), 3);
		assert_eq!(queue.known_headers[&100][&hash(100)], HeaderStatus::MaybeExtra);
		assert_eq!(queue.known_headers[&101][&hash(101)], HeaderStatus::MaybeExtra);
		assert_eq!(queue.known_headers[&102][&hash(102)], HeaderStatus::MaybeExtra);
	}

	#[test]
	fn negative_maybe_orphan_response_works() {
		// let's say we have:
		// #100 in MaybeOrphan
		// #101 in MaybeOrphan
		// and we have asked for MaybeOrphan status of #100.parent (i.e. #99)
		// and the response is: NO, #99 is NOT known to the Substrate runtime
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(100)
			.or_default()
			.insert(hash(100), header(100));
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(101)
			.or_default()
			.insert(hash(101), header(101));
		queue.maybe_orphan_response(&id(99), false);

		// then all headers (#100..#101) are moved to the Orphan queue
		assert!(queue.maybe_orphan.is_empty());
		assert_eq!(queue.orphan.len(), 2);
		assert_eq!(queue.known_headers[&100][&hash(100)], HeaderStatus::Orphan);
		assert_eq!(queue.known_headers[&101][&hash(101)], HeaderStatus::Orphan);
	}

	#[test]
	fn positive_maybe_extra_response_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeExtra);
		queue.maybe_extra.entry(100).or_default().insert(hash(100), header(100));
		queue.maybe_extra_response(&id(100), true);
		assert!(queue.maybe_extra.is_empty());
		assert_eq!(queue.extra.len(), 1);
		assert_eq!(queue.known_headers[&100][&hash(100)], HeaderStatus::Extra);
	}

	#[test]
	fn negative_maybe_extra_response_works() {
		// when parent header is complete
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::MaybeExtra);
		queue.maybe_extra.entry(100).or_default().insert(hash(100), header(100));
		queue.maybe_extra_response(&id(100), false);
		assert!(queue.maybe_extra.is_empty());
		assert_eq!(queue.ready.len(), 1);
		assert_eq!(queue.known_headers[&100][&hash(100)], HeaderStatus::Ready);

		// when parent header is incomplete
		queue.incomplete_headers.insert(id(200), None);
		queue
			.known_headers
			.entry(201)
			.or_default()
			.insert(hash(201), HeaderStatus::MaybeExtra);
		queue.maybe_extra.entry(201).or_default().insert(hash(201), header(201));
		queue.maybe_extra_response(&id(201), false);
		assert!(queue.maybe_extra.is_empty());
		assert_eq!(queue.incomplete.len(), 1);
		assert_eq!(queue.known_headers[&201][&hash(201)], HeaderStatus::Incomplete);
	}

	#[test]
	fn receipts_response_works() {
		// when parent header is complete
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Extra);
		queue.extra.entry(100).or_default().insert(hash(100), header(100));
		queue.extra_response(&id(100), 100_100);
		assert!(queue.extra.is_empty());
		assert_eq!(queue.ready.len(), 1);
		assert_eq!(queue.known_headers[&100][&hash(100)], HeaderStatus::Ready);

		// when parent header is incomplete
		queue.incomplete_headers.insert(id(200), None);
		queue
			.known_headers
			.entry(201)
			.or_default()
			.insert(hash(201), HeaderStatus::Extra);
		queue.extra.entry(201).or_default().insert(hash(201), header(201));
		queue.extra_response(&id(201), 201_201);
		assert!(queue.extra.is_empty());
		assert_eq!(queue.incomplete.len(), 1);
		assert_eq!(queue.known_headers[&201][&hash(201)], HeaderStatus::Incomplete);
	}

	#[test]
	fn header_submitted_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Ready);
		queue.ready.entry(100).or_default().insert(hash(100), header(100));
		queue.headers_submitted(vec![id(100)]);
		assert!(queue.ready.is_empty());
		assert_eq!(queue.known_headers[&100][&hash(100)], HeaderStatus::Submitted);
	}

	#[test]
	fn incomplete_header_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();

		// nothing to complete if queue is empty
		assert_eq!(queue.incomplete_header(), None);

		// when there's new header to complete => ask for completion data
		queue.incomplete_headers.insert(id(100), None);
		assert_eq!(queue.incomplete_header(), Some(id(100)));

		// we have just asked for completion data => nothing to request
		assert_eq!(queue.incomplete_header(), None);

		// enough time have passed => ask again
		queue.incomplete_headers.clear();
		queue.incomplete_headers.insert(
			id(100),
			Some(Instant::now() - RETRY_FETCH_COMPLETION_INTERVAL - RETRY_FETCH_COMPLETION_INTERVAL),
		);
		assert_eq!(queue.incomplete_header(), Some(id(100)));
	}

	#[test]
	fn completion_response_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue.incomplete_headers.insert(id(100), None);
		queue.incomplete_headers.insert(id(200), Some(Instant::now()));
		queue.incomplete_headers.insert(id(300), Some(Instant::now()));

		// when header isn't incompete, nothing changes
		queue.completion_response(&id(400), None);
		assert_eq!(queue.incomplete_headers.len(), 3);
		assert_eq!(queue.completion_data.len(), 0);
		assert_eq!(queue.header_to_complete(), None);

		// when response is None, nothing changes
		queue.completion_response(&id(100), None);
		assert_eq!(queue.incomplete_headers.len(), 3);
		assert_eq!(queue.completion_data.len(), 0);
		assert_eq!(queue.header_to_complete(), None);

		// when response is Some, we're scheduling completion
		queue.completion_response(&id(200), Some(200_200));
		assert_eq!(queue.completion_data.len(), 1);
		assert!(queue.completion_data.contains_key(&id(200)));
		assert_eq!(queue.header_to_complete(), Some((id(200), &200_200)));
		assert_eq!(
			queue.incomplete_headers.keys().collect::<Vec<_>>(),
			vec![&id(100), &id(300), &id(200)],
		);
	}

	#[test]
	fn header_completed_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue.completion_data.insert(id(100), 100_100);

		// when unknown header is completed
		queue.header_completed(&id(200));
		assert_eq!(queue.completion_data.len(), 1);

		// when known header is completed
		queue.header_completed(&id(100));
		assert_eq!(queue.completion_data.len(), 0);
	}

	#[test]
	fn incomplete_headers_response_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();

		// when we have already submitted #101 and #102 is ready
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Submitted);
		queue.submitted.entry(101).or_default().insert(hash(101), header(101));
		queue
			.known_headers
			.entry(102)
			.or_default()
			.insert(hash(102), HeaderStatus::Ready);
		queue.submitted.entry(102).or_default().insert(hash(102), header(102));

		// AND now we know that the #100 is incomplete
		queue.incomplete_headers_response(vec![id(100)].into_iter().collect());

		// => #101 and #102 are moved to the Incomplete and #100 is now synced
		assert_eq!(queue.status(&id(100)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(101)), HeaderStatus::Incomplete);
		assert_eq!(queue.status(&id(102)), HeaderStatus::Incomplete);
		assert_eq!(queue.submitted.len(), 0);
		assert_eq!(queue.ready.len(), 0);
		assert!(queue.incomplete.entry(101).or_default().contains_key(&hash(101)));
		assert!(queue.incomplete.entry(102).or_default().contains_key(&hash(102)));
		assert!(queue.incomplete_headers.contains_key(&id(100)));
		assert!(queue.completion_data.is_empty());

		// and then header #100 is no longer incomplete
		queue.incomplete_headers_response(vec![].into_iter().collect());

		// => #101 and #102 are moved to the Ready queue and #100 if now forgotten
		assert_eq!(queue.status(&id(100)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(101)), HeaderStatus::Ready);
		assert_eq!(queue.status(&id(102)), HeaderStatus::Ready);
		assert_eq!(queue.incomplete.len(), 0);
		assert_eq!(queue.submitted.len(), 0);
		assert!(queue.ready.entry(101).or_default().contains_key(&hash(101)));
		assert!(queue.ready.entry(102).or_default().contains_key(&hash(102)));
		assert!(queue.incomplete_headers.is_empty());
		assert!(queue.completion_data.is_empty());
	}

	#[test]
	fn is_parent_incomplete_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();

		// when we do not know header itself
		assert_eq!(queue.is_parent_incomplete(&id(50)), false);

		// when we do not know parent
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Incomplete);
		queue.incomplete.entry(100).or_default().insert(hash(100), header(100));
		assert_eq!(queue.is_parent_incomplete(&id(100)), false);

		// when parent is inside incomplete queue (i.e. some other ancestor is actually incomplete)
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Submitted);
		queue.submitted.entry(101).or_default().insert(hash(101), header(101));
		assert_eq!(queue.is_parent_incomplete(&id(101)), true);

		// when parent is the incomplete header and we do not have completion data
		queue.incomplete_headers.insert(id(199), None);
		queue
			.known_headers
			.entry(200)
			.or_default()
			.insert(hash(200), HeaderStatus::Submitted);
		queue.submitted.entry(200).or_default().insert(hash(200), header(200));
		assert_eq!(queue.is_parent_incomplete(&id(200)), true);

		// when parent is the incomplete header and we have completion data
		queue.completion_data.insert(id(299), 299_299);
		queue
			.known_headers
			.entry(300)
			.or_default()
			.insert(hash(300), HeaderStatus::Submitted);
		queue.submitted.entry(300).or_default().insert(hash(300), header(300));
		assert_eq!(queue.is_parent_incomplete(&id(300)), true);
	}

	#[test]
	fn prune_works() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();
		queue
			.known_headers
			.entry(105)
			.or_default()
			.insert(hash(105), HeaderStatus::Incomplete);
		queue.incomplete.entry(105).or_default().insert(hash(105), header(105));
		queue
			.known_headers
			.entry(104)
			.or_default()
			.insert(hash(104), HeaderStatus::MaybeOrphan);
		queue
			.maybe_orphan
			.entry(104)
			.or_default()
			.insert(hash(104), header(104));
		queue
			.known_headers
			.entry(103)
			.or_default()
			.insert(hash(103), HeaderStatus::Orphan);
		queue.orphan.entry(103).or_default().insert(hash(103), header(103));
		queue
			.known_headers
			.entry(102)
			.or_default()
			.insert(hash(102), HeaderStatus::MaybeExtra);
		queue.maybe_extra.entry(102).or_default().insert(hash(102), header(102));
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Extra);
		queue.extra.entry(101).or_default().insert(hash(101), header(101));
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Ready);
		queue.ready.entry(100).or_default().insert(hash(100), header(100));
		queue
			.synced_children
			.entry(100)
			.or_default()
			.insert(hash(100), vec![id(101)].into_iter().collect());
		queue
			.synced_children
			.entry(102)
			.or_default()
			.insert(hash(102), vec![id(102)].into_iter().collect());

		queue.prune(102);

		assert_eq!(queue.ready.len(), 0);
		assert_eq!(queue.extra.len(), 0);
		assert_eq!(queue.maybe_extra.len(), 1);
		assert_eq!(queue.orphan.len(), 1);
		assert_eq!(queue.maybe_orphan.len(), 1);
		assert_eq!(queue.incomplete.len(), 1);
		assert_eq!(queue.synced_children.len(), 1);
		assert_eq!(queue.known_headers.len(), 4);

		queue.prune(110);

		assert_eq!(queue.ready.len(), 0);
		assert_eq!(queue.extra.len(), 0);
		assert_eq!(queue.maybe_extra.len(), 0);
		assert_eq!(queue.orphan.len(), 0);
		assert_eq!(queue.maybe_orphan.len(), 0);
		assert_eq!(queue.incomplete.len(), 0);
		assert_eq!(queue.synced_children.len(), 0);
		assert_eq!(queue.known_headers.len(), 0);

		queue.header_response(header(109).header().clone());
		assert_eq!(queue.known_headers.len(), 0);

		queue.header_response(header(110).header().clone());
		assert_eq!(queue.known_headers.len(), 1);
	}

	#[test]
	fn incomplete_headers_are_still_incomplete_after_advance() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();

		// relay#1 knows that header#100 is incomplete && it has headers 101..104 in incomplete queue
		queue.incomplete_headers.insert(id(100), None);
		queue.incomplete.entry(101).or_default().insert(hash(101), header(101));
		queue.incomplete.entry(102).or_default().insert(hash(102), header(102));
		queue.incomplete.entry(103).or_default().insert(hash(103), header(103));
		queue.incomplete.entry(104).or_default().insert(hash(104), header(104));
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Synced);
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Incomplete);
		queue
			.known_headers
			.entry(102)
			.or_default()
			.insert(hash(102), HeaderStatus::Incomplete);
		queue
			.known_headers
			.entry(103)
			.or_default()
			.insert(hash(103), HeaderStatus::Incomplete);
		queue
			.known_headers
			.entry(104)
			.or_default()
			.insert(hash(104), HeaderStatus::Incomplete);

		// let's say relay#2 completes header#100 and then submits header#101+header#102 and it turns
		// out that header#102 is also incomplete
		queue.incomplete_headers_response(vec![id(102)].into_iter().collect());

		// then the header#103 and the header#104 must have Incomplete status
		assert_eq!(queue.status(&id(101)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(102)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(103)), HeaderStatus::Incomplete);
		assert_eq!(queue.status(&id(104)), HeaderStatus::Incomplete);
	}

	#[test]
	fn incomplete_headers_response_moves_synced_headers() {
		let mut queue = QueuedHeaders::<TestHeadersSyncPipeline>::default();

		// we have submitted two headers - 100 and 101. 102 is ready
		queue.submitted.entry(100).or_default().insert(hash(100), header(100));
		queue.submitted.entry(101).or_default().insert(hash(101), header(101));
		queue.ready.entry(102).or_default().insert(hash(102), header(102));
		queue
			.known_headers
			.entry(100)
			.or_default()
			.insert(hash(100), HeaderStatus::Submitted);
		queue
			.known_headers
			.entry(101)
			.or_default()
			.insert(hash(101), HeaderStatus::Submitted);
		queue
			.known_headers
			.entry(102)
			.or_default()
			.insert(hash(102), HeaderStatus::Ready);

		// both headers are accepted
		queue.target_best_header_response(&id(101));

		// but header 100 is incomplete
		queue.incomplete_headers_response(vec![id(100)].into_iter().collect());
		assert_eq!(queue.status(&id(100)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(101)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(102)), HeaderStatus::Incomplete);
		assert!(queue.incomplete_headers.contains_key(&id(100)));
		assert!(queue.incomplete[&102].contains_key(&hash(102)));

		// when header 100 is completed, 101 is synced and 102 is ready
		queue.incomplete_headers_response(HashSet::new());
		assert_eq!(queue.status(&id(100)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(101)), HeaderStatus::Synced);
		assert_eq!(queue.status(&id(102)), HeaderStatus::Ready);
		assert!(queue.ready[&102].contains_key(&hash(102)));
	}
}
