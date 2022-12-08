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

//! This module uses different approach for selecting dispute votes. It queries the Runtime
//! about the votes already known onchain and tries to select only relevant votes. Refer to
//! the documentation of `select_disputes` for more details about the actual implementation.

use crate::{error::GetOnchainDisputesError, metrics, LOG_TARGET};
use futures::channel::oneshot;
use polkadot_node_primitives::{dispute_is_inactive, CandidateVotes, DisputeStatus, Timestamp};
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{DisputeCoordinatorMessage, RuntimeApiMessage, RuntimeApiRequest},
	overseer, ActivatedLeaf,
};
use polkadot_primitives::v2::{
	supermajority_threshold, CandidateHash, DisputeState, DisputeStatement, DisputeStatementSet,
	Hash, MultiDisputeStatementSet, SessionIndex, ValidatorIndex,
};
use std::{
	collections::{BTreeMap, HashMap},
	time::{SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
mod tests;

/// The maximum number of disputes Provisioner will include in the inherent data.
/// Serves as a protection not to flood the Runtime with excessive data.
#[cfg(not(test))]
pub const MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME: usize = 200_000;
#[cfg(test)]
pub const MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME: usize = 200;

/// Controls how much dispute votes to be fetched from the `dispute-coordinator` per iteration in
/// `fn vote_selection`. The purpose is to fetch the votes in batches until
/// `MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME` is reached. If all votes are fetched in single call
/// we might fetch votes which we never use. This will create unnecessary load on `dispute-coordinator`.
///
/// This value should be less than `MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME`. Increase it in case
/// `provisioner` sends too many `QueryCandidateVotes` messages to `dispite-coordinator`.
#[cfg(not(test))]
const VOTES_SELECTION_BATCH_SIZE: usize = 1_100;
#[cfg(test)]
const VOTES_SELECTION_BATCH_SIZE: usize = 11;

/// Implements the `select_disputes` function which selects dispute votes which should
/// be sent to the Runtime.
///
/// # How the prioritization works
///
/// Generally speaking disputes can be described as:
///   * Active vs Inactive
///   * Known vs Unknown onchain
///   * Offchain vs Onchain
///   * Concluded onchain vs Unconcluded onchain
///
/// Provisioner fetches all disputes from `dispute-coordinator` and separates them in multiple partitions.
/// Please refer to `struct PartitionedDisputes` for details about the actual partitions.
/// Each partition has got a priority implicitly assigned to it and the disputes are selected based on this
/// priority (e.g. disputes in partition 1, then if there is space - disputes from partition 2 and so on).
///
/// # Votes selection
///
/// Besides the prioritization described above the votes in each partition are filtered too. Provisioner
/// fetches all onchain votes and filters them out from all partitions. As a result the Runtime receives
/// only fresh votes (votes it didn't know about).
///
/// # How the onchain votes are fetched
///
/// The logic outlined above relies on `RuntimeApiRequest::Disputes` message from the Runtime. The user
/// check the Runtime version before calling `select_disputes`. If the function is used with old runtime
/// an error is logged and the logic will continue with empty onchain votes `HashMap`.
pub async fn select_disputes<Sender>(
	sender: &mut Sender,
	metrics: &metrics::Metrics,
	leaf: &ActivatedLeaf,
) -> MultiDisputeStatementSet
where
	Sender: overseer::ProvisionerSenderTrait,
{
	gum::trace!(
		target: LOG_TARGET,
		?leaf,
		"Selecting disputes for inherent data using prioritized selection"
	);

	// Fetch the onchain disputes. We'll do a prioritization based on them.
	let onchain = match get_onchain_disputes(sender, leaf.hash).await {
		Ok(r) => r,
		Err(GetOnchainDisputesError::NotSupported(runtime_api_err, relay_parent)) => {
			// Runtime version is checked before calling this method, so the error below should never happen!
			gum::error!(
				target: LOG_TARGET,
				?runtime_api_err,
				?relay_parent,
				"Can't fetch onchain disputes, because ParachainHost runtime api version is old. Will continue with empty onchain disputes set.",
			);
			HashMap::new()
		},
		Err(GetOnchainDisputesError::Channel) => {
			// This error usually means the node is shutting down. Log just in case.
			gum::debug!(
				target: LOG_TARGET,
				"Channel error occurred while fetching onchain disputes. Will continue with empty onchain disputes set.",
			);
			HashMap::new()
		},
		Err(GetOnchainDisputesError::Execution(runtime_api_err, parent_hash)) => {
			gum::warn!(
				target: LOG_TARGET,
				?runtime_api_err,
				?parent_hash,
				"Unexpected execution error occurred while fetching onchain votes. Will continue with empty onchain disputes set.",
			);
			HashMap::new()
		},
	};

	let recent_disputes = request_disputes(sender).await;
	gum::trace!(
		target: LOG_TARGET,
		?leaf,
		"Got {} recent disputes and {} onchain disputes.",
		recent_disputes.len(),
		onchain.len(),
	);

	// Filter out unconfirmed disputes. However if the dispute is already onchain - don't skip it.
	// In this case we'd better push as much fresh votes as possible to bring it to conclusion faster.
	let recent_disputes = recent_disputes
		.into_iter()
		.filter(|d| d.2.is_confirmed_concluded() || onchain.contains_key(&(d.0, d.1)))
		.collect::<Vec<_>>();

	let partitioned = partition_recent_disputes(recent_disputes, &onchain);
	metrics.on_partition_recent_disputes(&partitioned);

	if partitioned.inactive_unknown_onchain.len() > 0 {
		gum::warn!(
			target: LOG_TARGET,
			?leaf,
			"Got {} inactive unknown onchain disputes. This should not happen!",
			partitioned.inactive_unknown_onchain.len()
		);
	}
	let result = vote_selection(sender, partitioned, &onchain).await;

	make_multi_dispute_statement_set(metrics, result)
}

/// Selects dispute votes from `PartitionedDisputes` which should be sent to the runtime. Votes which
/// are already onchain are filtered out. Result should be sorted by `(SessionIndex, CandidateHash)`
/// which is enforced by the `BTreeMap`. This is a requirement from the runtime.
async fn vote_selection<Sender>(
	sender: &mut Sender,
	partitioned: PartitionedDisputes,
	onchain: &HashMap<(SessionIndex, CandidateHash), DisputeState>,
) -> BTreeMap<(SessionIndex, CandidateHash), CandidateVotes>
where
	Sender: overseer::ProvisionerSenderTrait,
{
	// fetch in batches until there are enough votes
	let mut disputes = partitioned.into_iter().collect::<Vec<_>>();
	let mut total_votes_len = 0;
	let mut result = BTreeMap::new();
	let mut request_votes_counter = 0;
	while !disputes.is_empty() {
		let batch_size = std::cmp::min(VOTES_SELECTION_BATCH_SIZE, disputes.len());
		let batch = Vec::from_iter(disputes.drain(0..batch_size));

		// Filter votes which are already onchain
		request_votes_counter += 1;
		let votes = super::request_votes(sender, batch)
			.await
			.into_iter()
			.map(|(session_index, candidate_hash, mut votes)| {
				let onchain_state =
					if let Some(onchain_state) = onchain.get(&(session_index, candidate_hash)) {
						onchain_state
					} else {
						// onchain knows nothing about this dispute - add all votes
						return (session_index, candidate_hash, votes)
					};

				votes.valid.retain(|validator_idx, (statement_kind, _)| {
					is_vote_worth_to_keep(
						validator_idx,
						DisputeStatement::Valid(*statement_kind),
						&onchain_state,
					)
				});
				votes.invalid.retain(|validator_idx, (statement_kind, _)| {
					is_vote_worth_to_keep(
						validator_idx,
						DisputeStatement::Invalid(*statement_kind),
						&onchain_state,
					)
				});
				(session_index, candidate_hash, votes)
			})
			.collect::<Vec<_>>();

		// Check if votes are within the limit
		for (session_index, candidate_hash, selected_votes) in votes {
			let votes_len = selected_votes.valid.raw().len() + selected_votes.invalid.len();
			if votes_len + total_votes_len > MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME {
				// we are done - no more votes can be added
				return result
			}
			result.insert((session_index, candidate_hash), selected_votes);
			total_votes_len += votes_len
		}
	}

	gum::trace!(
		target: LOG_TARGET,
		?request_votes_counter,
		"vote_selection DisputeCoordinatorMessage::QueryCandidateVotes counter",
	);

	result
}

/// Contains disputes by partitions. Check the field comments for further details.
#[derive(Default)]
pub(crate) struct PartitionedDisputes {
	/// Concluded and inactive disputes which are completely unknown for the Runtime.
	/// Hopefully this should never happen.
	/// Will be sent to the Runtime with FIRST priority.
	pub inactive_unknown_onchain: Vec<(SessionIndex, CandidateHash)>,
	/// Disputes which are INACTIVE locally but they are unconcluded for the Runtime.
	/// A dispute can have enough local vote to conclude and at the same time the
	/// Runtime knows nothing about them at treats it as unconcluded. This discrepancy
	/// should be treated with high priority.
	/// Will be sent to the Runtime with SECOND priority.
	pub inactive_unconcluded_onchain: Vec<(SessionIndex, CandidateHash)>,
	/// Active disputes completely unknown onchain.
	/// Will be sent to the Runtime with THIRD priority.
	pub active_unknown_onchain: Vec<(SessionIndex, CandidateHash)>,
	/// Active disputes unconcluded onchain.
	/// Will be sent to the Runtime with FOURTH priority.
	pub active_unconcluded_onchain: Vec<(SessionIndex, CandidateHash)>,
	/// Active disputes concluded onchain. New votes are not that important for
	/// this partition.
	/// Will be sent to the Runtime with FIFTH priority.
	pub active_concluded_onchain: Vec<(SessionIndex, CandidateHash)>,
	/// Inactive disputes which has concluded onchain. These are not interesting and
	/// won't be sent to the Runtime.
	/// Will be DROPPED
	pub inactive_concluded_onchain: Vec<(SessionIndex, CandidateHash)>,
}

impl PartitionedDisputes {
	fn new() -> PartitionedDisputes {
		Default::default()
	}

	fn into_iter(self) -> impl Iterator<Item = (SessionIndex, CandidateHash)> {
		self.inactive_unknown_onchain
			.into_iter()
			.chain(self.inactive_unconcluded_onchain.into_iter())
			.chain(self.active_unknown_onchain.into_iter())
			.chain(self.active_unconcluded_onchain.into_iter())
			.chain(self.active_concluded_onchain.into_iter())
		// inactive_concluded_onchain is dropped on purpose
	}
}

fn secs_since_epoch() -> Timestamp {
	match SystemTime::now().duration_since(UNIX_EPOCH) {
		Ok(d) => d.as_secs(),
		Err(e) => {
			gum::warn!(
				target: LOG_TARGET,
				err = ?e,
				"Error getting system time."
			);
			0
		},
	}
}

fn concluded_onchain(onchain_state: &DisputeState) -> bool {
	// Check if there are enough onchain votes for or against to conclude the dispute
	let supermajority = supermajority_threshold(onchain_state.validators_for.len());

	onchain_state.validators_for.count_ones() >= supermajority ||
		onchain_state.validators_against.count_ones() >= supermajority
}

fn partition_recent_disputes(
	recent: Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
	onchain: &HashMap<(SessionIndex, CandidateHash), DisputeState>,
) -> PartitionedDisputes {
	let mut partitioned = PartitionedDisputes::new();

	// Drop any duplicates
	let unique_recent = recent
		.into_iter()
		.map(|(session_index, candidate_hash, dispute_state)| {
			((session_index, candidate_hash), dispute_state)
		})
		.collect::<HashMap<_, _>>();

	// Split recent disputes in ACTIVE and INACTIVE
	let time_now = &secs_since_epoch();
	let (active, inactive): (
		Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
		Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
	) = unique_recent
		.into_iter()
		.map(|((session_index, candidate_hash), dispute_state)| {
			(session_index, candidate_hash, dispute_state)
		})
		.partition(|(_, _, status)| !dispute_is_inactive(status, time_now));

	// Split ACTIVE in three groups...
	for (session_index, candidate_hash, _) in active {
		match onchain.get(&(session_index, candidate_hash)) {
			Some(d) => match concluded_onchain(d) {
				true => partitioned.active_concluded_onchain.push((session_index, candidate_hash)),
				false =>
					partitioned.active_unconcluded_onchain.push((session_index, candidate_hash)),
			},
			None => partitioned.active_unknown_onchain.push((session_index, candidate_hash)),
		};
	}

	// ... and INACTIVE in three more
	for (session_index, candidate_hash, _) in inactive {
		match onchain.get(&(session_index, candidate_hash)) {
			Some(onchain_state) =>
				if concluded_onchain(onchain_state) {
					partitioned.inactive_concluded_onchain.push((session_index, candidate_hash));
				} else {
					partitioned.inactive_unconcluded_onchain.push((session_index, candidate_hash));
				},
			None => partitioned.inactive_unknown_onchain.push((session_index, candidate_hash)),
		}
	}

	partitioned
}

/// Determines if a vote is worth to be kept, based on the onchain disputes
fn is_vote_worth_to_keep(
	validator_index: &ValidatorIndex,
	dispute_statement: DisputeStatement,
	onchain_state: &DisputeState,
) -> bool {
	let offchain_vote = match dispute_statement {
		DisputeStatement::Valid(_) => true,
		DisputeStatement::Invalid(_) => false,
	};
	let in_validators_for = onchain_state
		.validators_for
		.get(validator_index.0 as usize)
		.as_deref()
		.copied()
		.unwrap_or(false);
	let in_validators_against = onchain_state
		.validators_against
		.get(validator_index.0 as usize)
		.as_deref()
		.copied()
		.unwrap_or(false);

	if in_validators_for && in_validators_against {
		// The validator has double voted and runtime knows about this. Ignore this vote.
		return false
	}

	if offchain_vote && in_validators_against || !offchain_vote && in_validators_for {
		// offchain vote differs from the onchain vote
		// we need this vote to punish the offending validator
		return true
	}

	// The vote is valid. Return true if it is not seen onchain.
	!in_validators_for && !in_validators_against
}

/// Request disputes identified by `CandidateHash` and the `SessionIndex`.
async fn request_disputes(
	sender: &mut impl overseer::ProvisionerSenderTrait,
) -> Vec<(SessionIndex, CandidateHash, DisputeStatus)> {
	let (tx, rx) = oneshot::channel();
	let msg = DisputeCoordinatorMessage::RecentDisputes(tx);

	// Bounded by block production - `ProvisionerMessage::RequestInherentData`.
	sender.send_unbounded_message(msg);

	let recent_disputes = rx.await.unwrap_or_else(|err| {
		gum::warn!(target: LOG_TARGET, err=?err, "Unable to gather recent disputes");
		Vec::new()
	});
	recent_disputes
}

// This function produces the return value for `pub fn select_disputes()`
fn make_multi_dispute_statement_set(
	metrics: &metrics::Metrics,
	dispute_candidate_votes: BTreeMap<(SessionIndex, CandidateHash), CandidateVotes>,
) -> MultiDisputeStatementSet {
	// Transform all `CandidateVotes` into `MultiDisputeStatementSet`.
	dispute_candidate_votes
		.into_iter()
		.map(|((session_index, candidate_hash), votes)| {
			let valid_statements = votes
				.valid
				.into_iter()
				.map(|(i, (s, sig))| (DisputeStatement::Valid(s), i, sig));

			let invalid_statements = votes
				.invalid
				.into_iter()
				.map(|(i, (s, sig))| (DisputeStatement::Invalid(s), i, sig));

			metrics.inc_valid_statements_by(valid_statements.len());
			metrics.inc_invalid_statements_by(invalid_statements.len());
			metrics.inc_dispute_statement_sets_by(1);

			DisputeStatementSet {
				candidate_hash,
				session: session_index,
				statements: valid_statements.chain(invalid_statements).collect(),
			}
		})
		.collect()
}

/// Gets the on-chain disputes at a given block number and returns them as a `HashMap` so that searching in them is cheap.
pub async fn get_onchain_disputes<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
) -> Result<HashMap<(SessionIndex, CandidateHash), DisputeState>, GetOnchainDisputesError>
where
	Sender: overseer::ProvisionerSenderTrait,
{
	gum::trace!(target: LOG_TARGET, ?relay_parent, "Fetching on-chain disputes");
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::Disputes(tx)))
		.await;

	rx.await
		.map_err(|_| GetOnchainDisputesError::Channel)
		.and_then(|res| {
			res.map_err(|e| match e {
				RuntimeApiError::Execution { .. } =>
					GetOnchainDisputesError::Execution(e, relay_parent),
				RuntimeApiError::NotSupported { .. } =>
					GetOnchainDisputesError::NotSupported(e, relay_parent),
			})
		})
		.map(|v| v.into_iter().map(|e| ((e.0, e.1), e.2)).collect())
}
