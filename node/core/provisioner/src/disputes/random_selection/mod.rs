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

//! This module selects all RECENT disputes, fetches the votes for them from dispute-coordinator and
//! returns them as `MultiDisputeStatementSet`. If the RECENT disputes are more than
//! `MAX_DISPUTES_FORWARDED_TO_RUNTIME` constant - the ACTIVE disputes plus a random selection of
//! RECENT disputes (up to `MAX_DISPUTES_FORWARDED_TO_RUNTIME`) are returned instead.
//! If the ACTIVE disputes are also above `MAX_DISPUTES_FORWARDED_TO_RUNTIME` limit - a random selection
//! of them is generated.

use crate::{metrics, LOG_TARGET};
use futures::channel::oneshot;
use polkadot_node_subsystem::{messages::DisputeCoordinatorMessage, overseer};
use polkadot_primitives::v2::{
	CandidateHash, DisputeStatement, DisputeStatementSet, MultiDisputeStatementSet, SessionIndex,
};
use std::collections::HashSet;

/// The maximum number of disputes Provisioner will include in the inherent data.
/// Serves as a protection not to flood the Runtime with excessive data.
const MAX_DISPUTES_FORWARDED_TO_RUNTIME: usize = 1_000;

#[derive(Debug)]
enum RequestType {
	/// Query recent disputes, could be an excessive amount.
	Recent,
	/// Query the currently active and very recently concluded disputes.
	Active,
}

/// Request open disputes identified by `CandidateHash` and the `SessionIndex`.
/// Returns only confirmed/concluded disputes. The rest are filtered out.
async fn request_confirmed_disputes(
	sender: &mut impl overseer::ProvisionerSenderTrait,
	active_or_recent: RequestType,
) -> Vec<(SessionIndex, CandidateHash)> {
	let (tx, rx) = oneshot::channel();
	let msg = match active_or_recent {
		RequestType::Recent => DisputeCoordinatorMessage::RecentDisputes(tx),
		RequestType::Active => DisputeCoordinatorMessage::ActiveDisputes(tx),
	};

	sender.send_unbounded_message(msg);
	let disputes = match rx.await {
		Ok(r) => r,
		Err(oneshot::Canceled) => {
			gum::warn!(
				target: LOG_TARGET,
				"Channel closed: unable to gather {:?} disputes",
				active_or_recent
			);
			Vec::new()
		},
	};

	disputes
		.into_iter()
		.filter(|d| d.2.is_confirmed_concluded())
		.map(|d| (d.0, d.1))
		.collect()
}

/// Extend `acc` by `n` random, picks of not-yet-present in `acc` items of `recent` without repetition and additions of recent.
fn extend_by_random_subset_without_repetition(
	acc: &mut Vec<(SessionIndex, CandidateHash)>,
	extension: Vec<(SessionIndex, CandidateHash)>,
	n: usize,
) {
	use rand::Rng;

	let lut = acc.iter().cloned().collect::<HashSet<(SessionIndex, CandidateHash)>>();

	let mut unique_new =
		extension.into_iter().filter(|recent| !lut.contains(recent)).collect::<Vec<_>>();

	// we can simply add all
	if unique_new.len() <= n {
		acc.extend(unique_new)
	} else {
		acc.reserve(n);
		let mut rng = rand::thread_rng();
		for _ in 0..n {
			let idx = rng.gen_range(0..unique_new.len());
			acc.push(unique_new.swap_remove(idx));
		}
	}
	// assure sorting stays candid according to session index
	acc.sort_unstable_by(|a, b| a.0.cmp(&b.0));
}

pub async fn select_disputes<Sender>(
	sender: &mut Sender,
	metrics: &metrics::Metrics,
) -> MultiDisputeStatementSet
where
	Sender: overseer::ProvisionerSenderTrait,
{
	gum::trace!(target: LOG_TARGET, "Selecting disputes for inherent data using random selection");

	// We use `RecentDisputes` instead of `ActiveDisputes` because redundancy is fine.
	// It's heavier than `ActiveDisputes` but ensures that everything from the dispute
	// window gets on-chain, unlike `ActiveDisputes`.
	// In case of an overload condition, we limit ourselves to active disputes, and fill up to the
	// upper bound of disputes to pass to wasm `fn create_inherent_data`.
	// If the active ones are already exceeding the bounds, randomly select a subset.
	let recent = request_confirmed_disputes(sender, RequestType::Recent).await;
	let disputes = if recent.len() > MAX_DISPUTES_FORWARDED_TO_RUNTIME {
		gum::warn!(
			target: LOG_TARGET,
			"Recent disputes are excessive ({} > {}), reduce to active ones, and selected",
			recent.len(),
			MAX_DISPUTES_FORWARDED_TO_RUNTIME
		);
		let mut active = request_confirmed_disputes(sender, RequestType::Active).await;
		let n_active = active.len();
		let active = if active.len() > MAX_DISPUTES_FORWARDED_TO_RUNTIME {
			let mut picked = Vec::with_capacity(MAX_DISPUTES_FORWARDED_TO_RUNTIME);
			extend_by_random_subset_without_repetition(
				&mut picked,
				active,
				MAX_DISPUTES_FORWARDED_TO_RUNTIME,
			);
			picked
		} else {
			extend_by_random_subset_without_repetition(
				&mut active,
				recent,
				MAX_DISPUTES_FORWARDED_TO_RUNTIME.saturating_sub(n_active),
			);
			active
		};
		active
	} else {
		recent
	};

	// Load all votes for all disputes from the coordinator.
	let dispute_candidate_votes = super::request_votes(sender, disputes).await;

	// Transform all `CandidateVotes` into `MultiDisputeStatementSet`.
	dispute_candidate_votes
		.into_iter()
		.map(|(session_index, candidate_hash, votes)| {
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
