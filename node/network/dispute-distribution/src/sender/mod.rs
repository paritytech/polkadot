// Copyright 2021 Parity Technologies (UK) Ltd.
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

use std::collections::{hash_map::Entry, HashMap, HashSet};

use futures::channel::{mpsc, oneshot};

use polkadot_node_network_protocol::request_response::v1::DisputeRequest;
use polkadot_node_primitives::{CandidateVotes, DisputeMessage, SignedDisputeStatement};
use polkadot_node_subsystem::{messages::DisputeCoordinatorMessage, overseer, ActiveLeavesUpdate};
use polkadot_node_subsystem_util::runtime::RuntimeInfo;
use polkadot_primitives::v2::{CandidateHash, DisputeStatement, Hash, SessionIndex};

/// For each ongoing dispute we have a `SendTask` which takes care of it.
///
/// It is going to spawn real tasks as it sees fit for getting the votes of the particular dispute
/// out.
mod send_task;
use send_task::SendTask;
pub use send_task::TaskFinish;

/// Error and [`Result`] type for sender
mod error;
pub use error::{Error, FatalError, JfyiError, Result};

use self::error::JfyiErrorResult;
use crate::{Metrics, LOG_TARGET};

/// The `DisputeSender` keeps track of all ongoing disputes we need to send statements out.
///
/// For each dispute a `SendTask` is responsible for sending to the concerned validators for that
/// particular dispute. The `DisputeSender` keeps track of those tasks, informs them about new
/// sessions/validator sets and cleans them up when they become obsolete.
pub struct DisputeSender {
	/// All heads we currently consider active.
	active_heads: Vec<Hash>,

	/// List of currently active sessions.
	///
	/// Value is the hash that was used for the query.
	active_sessions: HashMap<SessionIndex, Hash>,

	/// All ongoing dispute sendings this subsystem is aware of.
	disputes: HashMap<CandidateHash, SendTask>,

	/// Sender to be cloned for `SendTask`s.
	tx: mpsc::Sender<TaskFinish>,

	/// Metrics for reporting stats about sent requests.
	metrics: Metrics,
}

#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
impl DisputeSender {
	/// Create a new `DisputeSender` which can be used to start dispute sendings.
	pub fn new(tx: mpsc::Sender<TaskFinish>, metrics: Metrics) -> Self {
		Self {
			active_heads: Vec::new(),
			active_sessions: HashMap::new(),
			disputes: HashMap::new(),
			tx,
			metrics,
		}
	}

	/// Create a `SendTask` for a particular new dispute.
	pub async fn start_sender<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		msg: DisputeMessage,
	) -> Result<()> {
		let req: DisputeRequest = msg.into();
		let candidate_hash = req.0.candidate_receipt.hash();
		match self.disputes.entry(candidate_hash) {
			Entry::Occupied(_) => {
				gum::trace!(target: LOG_TARGET, ?candidate_hash, "Dispute sending already active.");
				return Ok(())
			},
			Entry::Vacant(vacant) => {
				let send_task = SendTask::new(
					ctx,
					runtime,
					&self.active_sessions,
					self.tx.clone(),
					req,
					&self.metrics,
				)
				.await?;
				vacant.insert(send_task);
			},
		}
		Ok(())
	}

	/// Take care of a change in active leaves.
	///
	/// - Initiate a retry of failed sends which are still active.
	/// - Get new authorities to send messages to.
	/// - Get rid of obsolete tasks and disputes.
	/// - Get dispute sending started in case we missed one for some reason (e.g. on node startup)
	pub async fn update_leaves<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		update: ActiveLeavesUpdate,
	) -> Result<()> {
		let ActiveLeavesUpdate { activated, deactivated } = update;
		let deactivated: HashSet<_> = deactivated.into_iter().collect();
		self.active_heads.retain(|h| !deactivated.contains(h));
		self.active_heads.extend(activated.into_iter().map(|l| l.hash));

		let have_new_sessions = self.refresh_sessions(ctx, runtime).await?;

		let active_disputes = get_active_disputes(ctx).await?;
		let unknown_disputes = {
			let mut disputes = active_disputes.clone();
			disputes.retain(|(_, c)| !self.disputes.contains_key(c));
			disputes
		};

		let active_disputes: HashSet<_> = active_disputes.into_iter().map(|(_, c)| c).collect();

		// Cleanup obsolete senders:
		self.disputes
			.retain(|candidate_hash, _| active_disputes.contains(candidate_hash));

		for dispute in self.disputes.values_mut() {
			if have_new_sessions || dispute.has_failed_sends() {
				dispute
					.refresh_sends(ctx, runtime, &self.active_sessions, &self.metrics)
					.await?;
			}
		}

		// This should only be non-empty on startup, but if not - we got you covered:
		for dispute in unknown_disputes {
			self.start_send_for_dispute(ctx, runtime, dispute).await?
		}
		Ok(())
	}

	/// Receive message from a sending task.
	pub async fn on_task_message(&mut self, msg: TaskFinish) {
		let TaskFinish { candidate_hash, receiver, result } = msg;

		self.metrics.on_sent_request(result.as_metrics_label());

		let task = match self.disputes.get_mut(&candidate_hash) {
			None => {
				// Can happen when a dispute ends, with messages still in queue:
				gum::trace!(
					target: LOG_TARGET,
					?result,
					"Received `FromSendingTask::Finished` for non existing dispute."
				);
				return
			},
			Some(task) => task,
		};
		task.on_finished_send(&receiver, result);
	}

	/// Call `start_sender` on all passed in disputes.
	///
	/// Recover necessary votes for building up `DisputeMessage` and start sending for all of them.
	async fn start_send_for_dispute<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		dispute: (SessionIndex, CandidateHash),
	) -> Result<()> {
		let (session_index, candidate_hash) = dispute;
		// A relay chain head is required as context for receiving session info information from runtime and
		// storage. We will iterate `active_sessions` to find a suitable head. We assume that there is at
		// least one active head which, by `session_index`, is at least as recent as the `dispute` passed in.
		// We need to avoid picking an older one from a session that might not yet exist in storage.
		// Related to <https://github.com/paritytech/polkadot/issues/4730> .
		let ref_head = self
			.active_sessions
			.iter()
			.find_map(|(active_session_index, head_hash)| {
				// There might be more than one session index that is at least as recent as the dispute
				// so we just pick the first one. Keep in mind we are talking about the session index for the
				// child of block identified by `head_hash` and not the session index for the block.
				if active_session_index >= &session_index {
					Some(head_hash)
				} else {
					None
				}
			})
			.ok_or(JfyiError::NoActiveHeads)?;

		let info = runtime
			.get_session_info_by_index(ctx.sender(), *ref_head, session_index)
			.await?;
		let our_index = match info.validator_info.our_index {
			None => {
				gum::trace!(
					target: LOG_TARGET,
					"Not a validator in that session - not starting dispute sending."
				);
				return Ok(())
			},
			Some(index) => index,
		};

		let votes = match get_candidate_votes(ctx, session_index, candidate_hash).await? {
			None => {
				gum::debug!(
					target: LOG_TARGET,
					?session_index,
					?candidate_hash,
					"No votes for active dispute?! - possible, due to race."
				);
				return Ok(())
			},
			Some(votes) => votes,
		};

		let our_valid_vote = votes.valid.iter().find(|(_, i, _)| *i == our_index);

		let our_invalid_vote = votes.invalid.iter().find(|(_, i, _)| *i == our_index);

		let (valid_vote, invalid_vote) = if let Some(our_valid_vote) = our_valid_vote {
			// Get some invalid vote as well:
			let invalid_vote =
				votes.invalid.get(0).ok_or(JfyiError::MissingVotesFromCoordinator)?;
			(our_valid_vote, invalid_vote)
		} else if let Some(our_invalid_vote) = our_invalid_vote {
			// Get some valid vote as well:
			let valid_vote = votes.valid.get(0).ok_or(JfyiError::MissingVotesFromCoordinator)?;
			(valid_vote, our_invalid_vote)
		} else {
			// There is no vote from us yet - nothing to do.
			return Ok(())
		};
		let (kind, valid_index, signature) = valid_vote;
		let valid_public = info
			.session_info
			.validators
			.get(valid_index.0 as usize)
			.ok_or(JfyiError::InvalidStatementFromCoordinator)?;
		let valid_signed = SignedDisputeStatement::new_checked(
			DisputeStatement::Valid(kind.clone()),
			candidate_hash,
			session_index,
			valid_public.clone(),
			signature.clone(),
		)
		.map_err(|()| JfyiError::InvalidStatementFromCoordinator)?;

		let (kind, invalid_index, signature) = invalid_vote;
		let invalid_public = info
			.session_info
			.validators
			.get(invalid_index.0 as usize)
			.ok_or(JfyiError::InvalidValidatorIndexFromCoordinator)?;
		let invalid_signed = SignedDisputeStatement::new_checked(
			DisputeStatement::Invalid(kind.clone()),
			candidate_hash,
			session_index,
			invalid_public.clone(),
			signature.clone(),
		)
		.map_err(|()| JfyiError::InvalidValidatorIndexFromCoordinator)?;

		// Reconstructing the checked signed dispute statements is hardly useful here and wasteful,
		// but I don't want to enable a bypass for the below smart constructor and this code path
		// is supposed to be only hit on startup basically.
		//
		// Revisit this decision when the `from_signed_statements` is unneeded for the normal code
		// path as well.
		let message = DisputeMessage::from_signed_statements(
			valid_signed,
			*valid_index,
			invalid_signed,
			*invalid_index,
			votes.candidate_receipt,
			&info.session_info,
		)
		.map_err(JfyiError::InvalidDisputeFromCoordinator)?;

		// Finally, get the party started:
		self.start_sender(ctx, runtime, message).await
	}

	/// Make active sessions correspond to currently active heads.
	///
	/// Returns: true if sessions changed.
	async fn refresh_sessions<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
	) -> Result<bool> {
		let new_sessions = get_active_session_indices(ctx, runtime, &self.active_heads).await?;
		let new_sessions_raw: HashSet<_> = new_sessions.keys().collect();
		let old_sessions_raw: HashSet<_> = self.active_sessions.keys().collect();
		let updated = new_sessions_raw != old_sessions_raw;
		// Update in any case, so we use current heads for queries:
		self.active_sessions = new_sessions;
		Ok(updated)
	}
}

/// Retrieve the currently active sessions.
///
/// List is all indices of all active sessions together with the head that was used for the query.
#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
async fn get_active_session_indices<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	active_heads: &Vec<Hash>,
) -> Result<HashMap<SessionIndex, Hash>> {
	let mut indeces = HashMap::new();
	// Iterate all heads we track as active and fetch the child' session indices.
	for head in active_heads {
		let session_index = runtime.get_session_index_for_child(ctx.sender(), *head).await?;
		indeces.insert(session_index, *head);
	}
	Ok(indeces)
}

/// Retrieve Set of active disputes from the dispute coordinator.
#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
async fn get_active_disputes<Context>(
	ctx: &mut Context,
) -> JfyiErrorResult<Vec<(SessionIndex, CandidateHash)>> {
	let (tx, rx) = oneshot::channel();

	// Caller scope is in `update_leaves` and this is bounded by fork count.
	ctx.send_unbounded_message(DisputeCoordinatorMessage::ActiveDisputes(tx));
	rx.await.map_err(|_| JfyiError::AskActiveDisputesCanceled)
}

/// Get all locally available dispute votes for a given dispute.
#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
async fn get_candidate_votes<Context>(
	ctx: &mut Context,
	session_index: SessionIndex,
	candidate_hash: CandidateHash,
) -> JfyiErrorResult<Option<CandidateVotes>> {
	let (tx, rx) = oneshot::channel();
	// Caller scope is in `update_leaves` and this is bounded by fork count.
	ctx.send_unbounded_message(DisputeCoordinatorMessage::QueryCandidateVotes(
		vec![(session_index, candidate_hash)],
		tx,
	));
	rx.await
		.map(|v| v.get(0).map(|inner| inner.to_owned().2))
		.map_err(|_| JfyiError::AskCandidateVotesCanceled)
}
