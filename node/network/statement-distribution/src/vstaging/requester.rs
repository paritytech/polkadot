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

//! A requester for full information on candidates.
//!
// TODO [now]: some module docs.

use crate::LOG_TARGET;

use polkadot_node_network_protocol::{
	request_response::{
		outgoing::{Recipient as RequestRecipient, RequestError},
		vstaging::{AttestedCandidateRequest, AttestedCandidateResponse},
		OutgoingRequest, OutgoingResult, MAX_PARALLEL_ATTESTED_CANDIDATE_REQUESTS,
	},
	PeerId,
};
use polkadot_primitives::vstaging::{CandidateHash, GroupIndex, Hash, ParaId};

use bitvec::{order::Lsb0, vec::BitVec};
use futures::{channel::oneshot, prelude::*, stream::FuturesUnordered};

use std::{
	cmp::Reverse,
	collections::{
		hash_map::{Entry as HEntry, HashMap, VacantEntry},
		BTreeSet, HashSet, VecDeque,
	},
};

/// An identifier for a candidate.
///
/// In this module, we are requesting candidates
/// for which we have no information other than the candidate hash and statements signed
/// by validators. It is possible for validators for multiple groups to abuse this lack of
/// information: until we actually get the preimage of this candidate we cannot confirm
/// anything other than the candidate hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CandidateIdentifier {
	/// The relay-parent this candidate is ostensibly under.
	pub relay_parent: Hash,
	/// The hash of the candidate.
	pub candidate_hash: CandidateHash,
	/// The index of the group claiming to be assigned to the candidate's
	/// para.
	pub group_index: GroupIndex,
}

struct TaggedResponse {
	identifier: CandidateIdentifier,
	requested_peer: PeerId,
	response: AttestedCandidateResponse,
}

/// A pending request.
pub struct RequestedCandidate {
	priority: Priority,
	expected_para: ParaId,
	known_by: VecDeque<PeerId>,
	in_flight: bool,
}

impl RequestedCandidate {
	// TODO [now]: add peer to known set
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
	Cluster = 0,
	Unspecified = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Priority {
	origin: Origin,
	attempts: usize,
}

/// An entry for manipulating a requested candidate.
pub struct Entry<'a> {
	prev_index: usize,
	identifier: CandidateIdentifier,
	by_priority: &'a mut Vec<(Priority, CandidateIdentifier)>,
	requested: &'a mut RequestedCandidate,
}

impl<'a> Entry<'a> {
	/// Access the underlying requested candidate.
	pub fn get_mut(&mut self) -> &mut RequestedCandidate {
		&mut self.requested
	}
}

impl<'a> Drop for Entry<'a> {
	fn drop(&mut self) {
		insert_or_update_priority(
			&mut *self.by_priority,
			Some(self.prev_index),
			self.identifier.clone(),
			self.requested.priority.clone(),
		);
	}
}

/// A manager for outgoing requests.
pub struct RequestManager {
	pending_responses: FuturesUnordered<Box<dyn Future<Output = OutgoingResult<TaggedResponse>>>>,
	requests: HashMap<CandidateIdentifier, RequestedCandidate>,
	// sorted by priority.
	by_priority: Vec<(Priority, CandidateIdentifier)>,
	// all unique identifiers for the candidate.
	unique_identifiers: HashMap<CandidateHash, HashSet<CandidateIdentifier>>,
}

impl RequestManager {
	/// Create a new [`RequestManager`].
	pub fn new() -> Self {
		RequestManager {
			pending_responses: FuturesUnordered::new(),
			requests: HashMap::new(),
			by_priority: Vec::new(),
			unique_identifiers: HashMap::new(),
		}
	}

	/// Gets an [`Entry`] for mutating a request and inserts it if the
	/// manager doesn't store this request already.
	pub fn get_or_insert(
		&mut self,
		relay_parent: Hash,
		candidate_hash: CandidateHash,
		group_index: GroupIndex,
		group_assignment: ParaId,
	) -> Entry {
		let identifier = CandidateIdentifier { relay_parent, candidate_hash, group_index };

		let (candidate, fresh) = match self.requests.entry(identifier.clone()) {
			HEntry::Occupied(e) => (e.into_mut(), false),
			HEntry::Vacant(e) => (
				e.insert(RequestedCandidate {
					priority: Priority { attempts: 0, origin: Origin::Unspecified },
					expected_para: group_assignment,
					known_by: VecDeque::new(),
					in_flight: false,
				}),
				true,
			),
		};

		let priority_index = if fresh {
			self.unique_identifiers
				.entry(candidate_hash)
				.or_default()
				.insert(identifier.clone());

			insert_or_update_priority(
				&mut self.by_priority,
				None,
				identifier.clone(),
				candidate.priority.clone(),
			)
		} else {
			match self
				.by_priority
				.binary_search(&(candidate.priority.clone(), identifier.clone()))
			{
				Ok(i) => i,
				Err(i) => unreachable!("requested candidates always have a priority entry; qed"),
			}
		};

		Entry {
			prev_index: priority_index,
			identifier,
			by_priority: &mut self.by_priority,
			requested: candidate,
		}
	}

	/// Remove all pending requests for the given candidate.
	pub fn remove_for(&mut self, candidate: CandidateHash) {
		if let Some(identifiers) = self.unique_identifiers.remove(&candidate) {
			self.by_priority.retain(|(_priority, id)| !identifiers.contains(&id));
			for id in identifiers {
				self.requests.remove(&id);
			}
		}
	}

	// TODO [now]: removal based on relay-parent.

	/// Yields the next request to dispatch, if there is any.
	///
	/// This function accepts two closures as an argument.
	/// The first closure indicates whether a peer is still connected.
	/// The second closure is used to construct a mask for limiting the
	/// `Seconded` statements the response is allowed to contain.
	pub fn next_request(
		&mut self,
		peer_connected: impl Fn(&PeerId) -> bool,
		seconded_mask: impl Fn(&CandidateIdentifier) -> BitVec<u8, Lsb0>,
	) -> Option<OutgoingRequest<AttestedCandidateRequest>> {
		if self.pending_responses.len() >= MAX_PARALLEL_ATTESTED_CANDIDATE_REQUESTS as usize {
			return None
		}

		// loop over all requests, in order of priority.
		// do some active maintenance of the connected peers.
		// dispatch the first request which is not in-flight already.
		for (_priority, id) in &self.by_priority {
			let entry = match self.requests.get_mut(&id) {
				None => {
					gum::error!(
						target: LOG_TARGET,
						identifier = ?id,
						"Missing entry for priority queue member",
					);

					continue
				},
				Some(e) => e,
			};

			if entry.in_flight {
				continue
			}

			entry.known_by.retain(&peer_connected);

			let recipient = match entry.known_by.pop_front() {
				None => continue, // no peers.
				Some(r) => r,
			};

			entry.known_by.push_back(recipient.clone());

			let (request, response_fut) = OutgoingRequest::new(
				RequestRecipient::Peer(recipient.clone()),
				AttestedCandidateRequest {
					candidate_hash: id.candidate_hash,
					seconded_mask: seconded_mask(&id),
				},
			);

			let stored_id = id.clone();
			self.pending_responses.push(Box::new(async move {
				response_fut.await.map(|response| TaggedResponse {
					identifier: stored_id,
					requested_peer: recipient,
					response,
				})
			}));

			entry.in_flight = true;

			return Some(request)
		}

		None
	}

	// TODO [now]: `await_incoming -> IncomingPendingValidation`
}

fn insert_or_update_priority(
	priority_sorted: &mut Vec<(Priority, CandidateIdentifier)>,
	prev_index: Option<usize>,
	candidate_identifier: CandidateIdentifier,
	new_priority: Priority,
) -> usize {
	if let Some(prev_index) = prev_index {
		// GIGO: this behaves strangely if prev-index is not for the
		// expected identifier.
		if priority_sorted[prev_index].0 == new_priority {
			// unchanged.
			return prev_index
		} else {
			priority_sorted.remove(prev_index);
		}
	}

	let item = (new_priority, candidate_identifier);
	match priority_sorted.binary_search(&item) {
		Ok(i) => i, // ignore if already present.
		Err(i) => {
			priority_sorted.insert(i, item);
			i
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// TODO [now]: test priority ordering.
}
