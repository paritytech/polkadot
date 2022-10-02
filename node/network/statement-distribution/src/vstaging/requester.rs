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

use polkadot_node_network_protocol::{
	request_response::{
		vstaging::{AttestedCandidateRequest, AttestedCandidateResponse},
		MAX_PARALLEL_ATTESTED_CANDIDATE_REQUESTS,
	},
	PeerId,
};
use polkadot_primitives::vstaging::{
	AuthorityDiscoveryId, CandidateHash, GroupIndex, Hash, ParaId,
};

use bitvec::{order::Lsb0, vec::BitVec};
use futures::{channel::oneshot, prelude::*, stream::FuturesUnordered};

use std::collections::{
	hash_map::{Entry as HEntry, HashMap, VacantEntry},
	HashSet,
};

/// An identifier for a candidate.
///
/// In this module, we are requesting candidates
/// for which we have no information other than the candidate hash and statements signed
/// by validators. It is possible for validators for multiple groups to abuse this lack of
/// information: until we actually get the preimage of this candidate we cannot confirm
/// anything other than the candidate hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateIdentifier {
	relay_parent: Hash,
	candidate_hash: CandidateHash,
	group_index: GroupIndex,
}

struct TaggedResponse {
	identifier: CandidateIdentifier,
	authority_id: AuthorityDiscoveryId,
	response: AttestedCandidateResponse,
}

/// A pending request.
pub struct PendingRequest {
	expected_para: ParaId,
	known_by: Vec<AuthorityDiscoveryId>,
}

/// A vacant pending request entry.
pub struct VacantRequestEntry<'a> {
	vacant_request_entry: VacantEntry<'a, CandidateIdentifier, PendingRequest>,
	unique_identifiers: &'a mut HashMap<CandidateHash, HashSet<CandidateIdentifier>>,
	identifier: CandidateIdentifier,
}

/// An entry in the request manager.
pub enum RequestEntry<'a> {
	Occupied(&'a mut PendingRequest),
	Vacant(VacantRequestEntry<'a>),
}

impl<'a> RequestEntry<'a> {
	/// Yields the existing pending request or inserts it, with the given
	/// metadata.
	pub fn or_insert_with(
		self,
		group_assignments: impl FnOnce(GroupIndex) -> ParaId,
	) -> &'a mut PendingRequest {
		let mut vacant = match self {
			RequestEntry::Occupied(o) => return o,
			RequestEntry::Vacant(v) => v,
		};

		vacant
			.unique_identifiers
			.entry(vacant.identifier.candidate_hash)
			.or_insert_with(HashSet::new)
			.insert(vacant.identifier.clone());

		let group_index = vacant.identifier.group_index;

		vacant.vacant_request_entry.insert(PendingRequest {
			expected_para: group_assignments(group_index),
			known_by: Vec::new(),
		})
	}
}

/// A manager for outgoing requests.
pub struct RequestManager {
	pending_responses: FuturesUnordered<Box<dyn Future<Output = TaggedResponse>>>,
	requests: HashMap<CandidateIdentifier, PendingRequest>,
	// all unique identifiers for the candidate.
	unique_identifiers: HashMap<CandidateHash, HashSet<CandidateIdentifier>>,
}

impl RequestManager {
	/// Create a new [`RequestManager`].
	pub fn new() -> Self {
		RequestManager {
			pending_responses: FuturesUnordered::new(),
			requests: HashMap::new(),
			unique_identifiers: HashMap::new(),
		}
	}

	/// Either yields the pending request data for the given parameters,
	/// or yields a [`VacantRequestEntry`] which can be used to instantiate
	/// it.
	pub fn entry(
		&mut self,
		relay_parent: Hash,
		candidate_hash: CandidateHash,
		group_index: GroupIndex,
	) -> RequestEntry {
		let identifier = CandidateIdentifier { relay_parent, candidate_hash, group_index };

		let requests = &mut self.requests;
		let unique_identifiers = &mut self.unique_identifiers;

		match requests.entry(identifier.clone()) {
			HEntry::Vacant(v) => RequestEntry::Vacant(VacantRequestEntry {
				vacant_request_entry: v,
				unique_identifiers,
				identifier,
			}),
			HEntry::Occupied(o) => RequestEntry::Occupied(o.into_mut()),
		}
	}

	/// Remove all pending requests for the given candidate.
	pub fn remove_for(&mut self, candidate: CandidateHash) {
		if let Some(identifiers) = self.unique_identifiers.remove(&candidate) {
			for id in identifiers {
				self.requests.remove(&id);
			}
		}
	}

	// TODO [now]: `dispatch_next -> Option<Requests>`

	// TODO [now]: `await_incoming -> IncomingPendingValidation`
}
