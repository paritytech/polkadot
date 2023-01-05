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

use std::{
	collections::{HashMap, HashSet},
	pin::Pin,
	task::Poll,
	time::Duration,
};

use futures::{
	channel::{mpsc, oneshot},
	future::poll_fn,
	Future,
};

use futures_timer::Delay;
use indexmap::{map::Entry, IndexMap};
use polkadot_node_network_protocol::request_response::v1::DisputeRequest;
use polkadot_node_primitives::{CandidateVotes, DisputeMessage, SignedDisputeStatement};
use polkadot_node_subsystem::{messages::DisputeCoordinatorMessage, overseer, ActiveLeavesUpdate};
use polkadot_node_subsystem_util::runtime::RuntimeInfo;
use polkadot_primitives::v2::{CandidateHash, DisputeStatement, Hash, SessionIndex};

/// For each ongoing dispute we have a `SendTask` which takes care of it.
///
/// It is going to spawn real tasks as it sees fit for getting the votes of the particular dispute
/// out.
///
/// As we assume disputes have a priority, we start sending for disputes in the order
/// `start_sender` got called.
mod send_task;
use send_task::SendTask;
pub use send_task::TaskFinish;

/// Error and [`Result`] type for sender.
mod error;
pub use error::{Error, FatalError, JfyiError, Result};

use self::error::JfyiErrorResult;
use crate::{Metrics, LOG_TARGET, SEND_RATE_LIMIT};

/// The `DisputeSender` keeps track of all ongoing disputes we need to send statements out.
///
/// For each dispute a `SendTask` is responsible for sending to the concerned validators for that
/// particular dispute. The `DisputeSender` keeps track of those tasks, informs them about new
/// sessions/validator sets and cleans them up when they become obsolete.
///
/// The unit of work for the  `DisputeSender` is a dispute, represented by `SendTask`s.
pub struct DisputeSender {
	/// All heads we currently consider active.
	active_heads: Vec<Hash>,

	/// List of currently active sessions.
	///
	/// Value is the hash that was used for the query.
	active_sessions: HashMap<SessionIndex, Hash>,

	/// All ongoing dispute sendings this subsystem is aware of.
	///
	/// Using an `IndexMap` so items can be iterated in the order of insertion.
	disputes: IndexMap<CandidateHash, SendTask>,

	/// Sender to be cloned for `SendTask`s.
	tx: mpsc::Sender<TaskFinish>,

	/// Future for delaying too frequent creation of dispute sending tasks.
	rate_limit: RateLimit,

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
			disputes: IndexMap::new(),
			tx,
			rate_limit: RateLimit::new(),
			metrics,
		}
	}

	/// Create a `SendTask` for a particular new dispute.
	///
	/// This function is rate-limited by `SEND_RATE_LIMIT`. It will block if called too frequently
	/// in order to maintain the limit.
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
				self.rate_limit.limit("in start_sender", candidate_hash).await;

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
	///
	/// This function ensures the `SEND_RATE_LIMIT`, therefore it might block.
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

		// Cleanup obsolete senders (retain keeps order of remaining elements):
		self.disputes
			.retain(|candidate_hash, _| active_disputes.contains(candidate_hash));

		// Iterates in order of insertion:
		let mut should_rate_limit = true;
		for (candidate_hash, dispute) in self.disputes.iter_mut() {
			if have_new_sessions || dispute.has_failed_sends() {
				if should_rate_limit {
					self.rate_limit
						.limit("while going through new sessions/failed sends", *candidate_hash)
						.await;
				}
				let sends_happened = dispute
					.refresh_sends(ctx, runtime, &self.active_sessions, &self.metrics)
					.await?;
				// Only rate limit if we actually sent something out _and_ it was not just because
				// of errors on previous sends.
				//
				// Reasoning: It would not be acceptable to slow down the whole subsystem, just
				// because of a few bad peers having problems. It is actually better to risk
				// running into their rate limit in that case and accept a minor reputation change.
				should_rate_limit = sends_happened && have_new_sessions;
			}
		}

		// This should only be non-empty on startup, but if not - we got you covered.
		//
		// Initial order will not be maintained in that case, but that should be fine as disputes
		// recovered at startup will be relatively "old" anyway and we assume that no more than a
		// third of the validators will go offline at any point in time anyway.
		for dispute in unknown_disputes {
			// Rate limiting handled inside `start_send_for_dispute` (calls `start_sender`).
			self.start_send_for_dispute(ctx, runtime, dispute).await?;
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

		let our_valid_vote = votes.valid.raw().get(&our_index);

		let our_invalid_vote = votes.invalid.get(&our_index);

		let (valid_vote, invalid_vote) = if let Some(our_valid_vote) = our_valid_vote {
			// Get some invalid vote as well:
			let invalid_vote =
				votes.invalid.iter().next().ok_or(JfyiError::MissingVotesFromCoordinator)?;
			((&our_index, our_valid_vote), invalid_vote)
		} else if let Some(our_invalid_vote) = our_invalid_vote {
			// Get some valid vote as well:
			let valid_vote =
				votes.valid.raw().iter().next().ok_or(JfyiError::MissingVotesFromCoordinator)?;
			(valid_vote, (&our_index, our_invalid_vote))
		} else {
			// There is no vote from us yet - nothing to do.
			return Ok(())
		};
		let (valid_index, (kind, signature)) = valid_vote;
		let valid_public = info
			.session_info
			.validators
			.get(*valid_index)
			.ok_or(JfyiError::InvalidStatementFromCoordinator)?;
		let valid_signed = SignedDisputeStatement::new_checked(
			DisputeStatement::Valid(*kind),
			candidate_hash,
			session_index,
			valid_public.clone(),
			signature.clone(),
		)
		.map_err(|()| JfyiError::InvalidStatementFromCoordinator)?;

		let (invalid_index, (kind, signature)) = invalid_vote;
		let invalid_public = info
			.session_info
			.validators
			.get(*invalid_index)
			.ok_or(JfyiError::InvalidValidatorIndexFromCoordinator)?;
		let invalid_signed = SignedDisputeStatement::new_checked(
			DisputeStatement::Invalid(*kind),
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

/// Rate limiting logic.
///
/// Suitable for the sending side.
struct RateLimit {
	limit: Delay,
}

impl RateLimit {
	/// Create new `RateLimit` that is immediately ready.
	fn new() -> Self {
		// Start with an empty duration, as there has not been any previous call.
		Self { limit: Delay::new(Duration::new(0, 0)) }
	}

	/// Initialized with actual `SEND_RATE_LIMIT` duration.
	fn new_limit() -> Self {
		Self { limit: Delay::new(SEND_RATE_LIMIT) }
	}

	/// Wait until ready and prepare for next call.
	///
	/// String given as occasion and candidate hash are logged in case the rate limit hit.
	async fn limit(&mut self, occasion: &'static str, candidate_hash: CandidateHash) {
		// Wait for rate limit and add some logging:
		let mut num_wakes: u32 = 0;
		poll_fn(|cx| {
			let old_limit = Pin::new(&mut self.limit);
			match old_limit.poll(cx) {
				Poll::Pending => {
					gum::debug!(
						target: LOG_TARGET,
						?occasion,
						?candidate_hash,
						?num_wakes,
						"Sending rate limit hit, slowing down requests"
					);
					num_wakes += 1;
					Poll::Pending
				},
				Poll::Ready(()) => Poll::Ready(()),
			}
		})
		.await;
		*self = Self::new_limit();
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
	rx.await
		.map_err(|_| JfyiError::AskActiveDisputesCanceled)
		.map(|disputes| disputes.into_iter().map(|d| (d.0, d.1)).collect())
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
