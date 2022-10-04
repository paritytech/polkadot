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

use super::{
	BENEFIT_VALID_RESPONSE, BENEFIT_VALID_STATEMENT, COST_IMPROPERLY_DECODED_RESPONSE,
	COST_INVALID_RESPONSE, COST_INVALID_SIGNATURE, COST_UNREQUESTED_RESPONSE_STATEMENT,
};
use crate::LOG_TARGET;

use polkadot_node_network_protocol::{
	request_response::{
		outgoing::{Recipient as RequestRecipient, RequestError},
		vstaging::{AttestedCandidateRequest, AttestedCandidateResponse},
		OutgoingRequest, OutgoingResult, MAX_PARALLEL_ATTESTED_CANDIDATE_REQUESTS,
	},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::vstaging::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, GroupIndex, Hash, ParaId,
	PersistedValidationData, SessionIndex, SignedStatement, SigningContext, ValidatorId,
	ValidatorIndex,
};

use bitvec::{order::Lsb0, vec::BitVec};
use futures::{channel::oneshot, future::BoxFuture, prelude::*, stream::FuturesUnordered};

use std::{
	collections::{
		hash_map::{Entry as HEntry, HashMap, VacantEntry},
		BTreeSet, HashSet, VecDeque,
	},
	pin::Pin,
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
	seconded_mask: BitVec<u8, Lsb0>,
	response: OutgoingResult<AttestedCandidateResponse>,
}

/// A pending request.
pub struct RequestedCandidate {
	priority: Priority,
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
	pending_responses: FuturesUnordered<BoxFuture<'static, TaggedResponse>>,
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
	) -> Entry {
		let identifier = CandidateIdentifier { relay_parent, candidate_hash, group_index };

		let (candidate, fresh) = match self.requests.entry(identifier.clone()) {
			HEntry::Occupied(e) => (e.into_mut(), false),
			HEntry::Vacant(e) => (
				e.insert(RequestedCandidate {
					priority: Priority { attempts: 0, origin: Origin::Unspecified },
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

			let seconded_mask = seconded_mask(&id);
			let (request, response_fut) = OutgoingRequest::new(
				RequestRecipient::Peer(recipient.clone()),
				AttestedCandidateRequest {
					candidate_hash: id.candidate_hash,
					seconded_mask: seconded_mask.clone(),
				},
			);

			let stored_id = id.clone();
			self.pending_responses.push(Box::pin(async move {
				TaggedResponse {
					identifier: stored_id,
					requested_peer: recipient,
					seconded_mask,
					response: response_fut.await,
				}
			}));

			entry.in_flight = true;

			return Some(request)
		}

		None
	}

	/// Await the next incoming response to a sent request, or immediately
	/// return `None` if there are no pending responses.
	pub async fn await_incoming(&mut self) -> Option<UnhandledResponse> {
		match self.pending_responses.next().await {
			None => None,
			Some(response) => Some(UnhandledResponse { manager: self, response }),
		}
	}
}

/// A response to a request, which has not yet been handled.
pub struct UnhandledResponse<'a> {
	manager: &'a mut RequestManager,
	response: TaggedResponse,
}

impl<'a> UnhandledResponse<'a> {
	/// Get the candidate identifier which the corresponding request
	/// was classified under.
	pub fn candidate_identifier(&self) -> &CandidateIdentifier {
		&self.response.identifier
	}

	/// Validate the response. If the response is valid, this will yield the
	/// candidate, the [`PersistedValidationData`] of the candidate, and requested
	/// checked statements.
	///
	/// This will also produce a record of misbehaviors by peers:
	///   * If the response is partially valid, misbehavior by the responding peer.
	///   * If there are other peers which have advertised the same candidate for different
	///     relay-parents or para-ids, misbehavior reports for those peers will also
	///     be generated.
	///
	/// Finally, in the case that the response is either valid or partially valid,
	/// this will clean up all remaining requests for the candidate in the manager.
	///
	/// As parameters, the user should supply the canonical group array as well
	/// as a mapping from validator index to validator ID. The validator pubkey mapping
	/// will not be queried except for validator indices in the group.
	pub fn validate_response(
		self,
		group: &[ValidatorIndex],
		session: SessionIndex,
		validator_key_lookup: impl Fn(ValidatorIndex) -> ValidatorId,
		allowed_para_lookup: impl Fn(ParaId) -> bool,
	) -> ResponseValidationOutput {
		let UnhandledResponse {
			manager,
			response: TaggedResponse { identifier, requested_peer, response, seconded_mask },
		} = self;

		// handle races if the candidate is no longer known.
		// this could happen if we requested the candidate under two
		// different identifiers at the same time, and received a valid
		// response on the other.
		let entry = match manager.requests.get_mut(&identifier) {
			None =>
				return ResponseValidationOutput {
					reputation_changes: Vec::new(),
					request_status: CandidateRequestStatus::Outdated,
				},
			Some(e) => e,
		};

		let priority_index = match manager
			.by_priority
			.binary_search(&(entry.priority.clone(), identifier.clone()))
		{
			Ok(i) => i,
			Err(i) => unreachable!("requested candidates always have a priority entry; qed"),
		};

		entry.in_flight = false;
		entry.priority.attempts += 1;

		// update the location in the priority queue.
		insert_or_update_priority(
			&mut manager.by_priority,
			Some(priority_index),
			identifier.clone(),
			entry.priority.clone(),
		);

		let complete_response = match response {
			Err(RequestError::InvalidResponse(e)) => {
				gum::trace!(
					target: LOG_TARGET,
					err = ?e,
					peer = ?requested_peer,
					"Improperly encoded response"
				);

				return ResponseValidationOutput {
					reputation_changes: vec![(requested_peer, COST_IMPROPERLY_DECODED_RESPONSE)],
					request_status: CandidateRequestStatus::Incomplete,
				}
			},
			Err(RequestError::NetworkError(_) | RequestError::Canceled(_)) =>
				return ResponseValidationOutput {
					reputation_changes: vec![],
					request_status: CandidateRequestStatus::Incomplete,
				},
			Ok(response) => response,
		};

		let mut output = validate_complete_response(
			&identifier,
			complete_response,
			requested_peer,
			seconded_mask,
			group,
			session,
			validator_key_lookup,
		);

		if let CandidateRequestStatus::Complete { .. } = output.request_status {
			// TODO [now]: clean up everything else to do with the candidate.
			// add reputation punishments for all peers advertising the candidate under
			// different identifiers.
		}

		output
	}
}

fn validate_complete_response(
	identifier: &CandidateIdentifier,
	response: AttestedCandidateResponse,
	requested_peer: PeerId,
	mut sent_seconded_bitmask: BitVec<u8, Lsb0>,
	group: &[ValidatorIndex],
	session: SessionIndex,
	validator_key_lookup: impl Fn(ValidatorIndex) -> ValidatorId,
) -> ResponseValidationOutput {
	// sanity check bitmask size. this is based entirely on
	// local logic here.
	if sent_seconded_bitmask.len() != group.len() {
		gum::error!(
			target: LOG_TARGET,
			group_len = group.len(),
			sent_bitmask_len = sent_seconded_bitmask.len(),
			"Logic bug: group size != sent bitmask len"
		);

		// resize and attempt to continue.
		sent_seconded_bitmask.resize(group.len(), true);
	}

	let invalid_candidate_output = || ResponseValidationOutput {
		request_status: CandidateRequestStatus::Incomplete,
		reputation_changes: vec![(requested_peer.clone(), COST_INVALID_RESPONSE)],
	};

	// sanity-check candidate response.
	// note: roughly ascending cost of operations
	{
		if response.candidate_receipt.descriptor.relay_parent != identifier.relay_parent {
			return invalid_candidate_output()
		}

		if response.candidate_receipt.descriptor.persisted_validation_data_hash !=
			response.persisted_validation_data.hash()
		{
			return invalid_candidate_output()
		}

		if response.candidate_receipt.hash() != identifier.candidate_hash {
			return invalid_candidate_output()
		}
	}

	// statement checks.
	let mut rep_changes = Vec::new();
	let statements = {
		let mut statements =
			Vec::with_capacity(std::cmp::min(response.statements.len(), group.len() * 2));

		let mut received_seconded = BitVec::<usize, Lsb0>::repeat(false, group.len());
		let mut received_valid = BitVec::<usize, Lsb0>::repeat(false, group.len());

		let index_in_group = |v: ValidatorIndex| group.iter().position(|x| &v == x);

		let signing_context =
			SigningContext { parent_hash: identifier.relay_parent, session_index: session };

		for unchecked_statement in response.statements.into_iter().take(group.len() * 2) {
			// ensure statement is from a validator in the group.
			let i = match index_in_group(unchecked_statement.unchecked_validator_index()) {
				Some(i) => i,
				None => {
					rep_changes.push((requested_peer.clone(), COST_UNREQUESTED_RESPONSE_STATEMENT));
					continue
				},
			};

			// ensure statement is on the correct candidate hash.
			if unchecked_statement.unchecked_payload().candidate_hash() !=
				&identifier.candidate_hash
			{
				rep_changes.push((requested_peer.clone(), COST_UNREQUESTED_RESPONSE_STATEMENT));
				continue
			}

			// filter out duplicates or statements outside the mask.
			// note on indexing: we have ensured that the bitmask and the
			// duplicate trackers have the correct size for the group.
			match unchecked_statement.unchecked_payload() {
				CompactStatement::Seconded(_) => {
					if !sent_seconded_bitmask[i] {
						rep_changes
							.push((requested_peer.clone(), COST_UNREQUESTED_RESPONSE_STATEMENT));
						continue
					}

					if received_seconded[i] {
						rep_changes
							.push((requested_peer.clone(), COST_UNREQUESTED_RESPONSE_STATEMENT));
						continue
					}
				},
				CompactStatement::Valid(_) =>
					if received_valid[i] {
						rep_changes
							.push((requested_peer.clone(), COST_UNREQUESTED_RESPONSE_STATEMENT));
						continue
					},
			}

			let validator_public =
				validator_key_lookup(unchecked_statement.unchecked_validator_index());
			let checked_statement =
				match unchecked_statement.try_into_checked(&signing_context, &validator_public) {
					Err(_) => {
						rep_changes.push((requested_peer.clone(), COST_INVALID_SIGNATURE));
						continue
					},
					Ok(checked) => checked,
				};

			match checked_statement.payload() {
				CompactStatement::Seconded(_) => {
					received_seconded.set(i, true);
				},
				CompactStatement::Valid(_) => {
					received_valid.set(i, true);
				},
			}

			statements.push(checked_statement);
			rep_changes.push((requested_peer.clone(), BENEFIT_VALID_STATEMENT));
		}

		statements
	};

	rep_changes.push((requested_peer.clone(), BENEFIT_VALID_RESPONSE));

	ResponseValidationOutput {
		request_status: CandidateRequestStatus::Complete {
			candidate: response.candidate_receipt,
			persisted_validation_data: response.persisted_validation_data,
			statements,
		},
		reputation_changes: rep_changes,
	}
}

/// The status of the candidate request after the handling of a response.
pub enum CandidateRequestStatus {
	/// The request was outdated at the point of receiving the response.
	Outdated,
	/// The response either did not arrive or was invalid.
	Incomplete,
	/// The response completed the request. Statements sent beyond the
	/// mask have been ignored. More statements which may have been
	/// expected may not be present, and higher-level code should
	/// evaluate whether the candidate is still worth storing and whether
	/// the sender should be punished.
	///
	/// This also does not indicate that the para has actually been checked
	/// to be one that the group is assigned under. Higher-level code should
	/// verify that this is the case and ignore the candidate accordingly if so.
	Complete {
		candidate: CommittedCandidateReceipt,
		persisted_validation_data: PersistedValidationData,
		statements: Vec<SignedStatement>,
	},
}

/// Output of the response validation.
pub struct ResponseValidationOutput {
	/// The status of the request.
	pub request_status: CandidateRequestStatus,
	/// Any reputation changes as a result of validating the response.
	pub reputation_changes: Vec<(PeerId, Rep)>,
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
