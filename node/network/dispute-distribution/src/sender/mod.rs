// Copyright (C) Parity Technologies (UK) Ltd.
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

use futures::{channel::oneshot, future::poll_fn, Future};

use futures_timer::Delay;
use indexmap::{map::Entry, IndexMap};
use polkadot_node_network_protocol::request_response::v1::DisputeRequest;
use polkadot_node_primitives::{DisputeMessage, DisputeStatus};
use polkadot_node_subsystem::{
	messages::DisputeCoordinatorMessage, overseer, ActiveLeavesUpdate, SubsystemSender,
};
use polkadot_node_subsystem_util::{nesting_sender::NestingSender, runtime::RuntimeInfo};
use polkadot_primitives::{CandidateHash, Hash, SessionIndex};

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

/// Messages as sent by background tasks.
#[derive(Debug)]
pub enum DisputeSenderMessage {
	/// A task finished.
	TaskFinish(TaskFinish),
	/// A request for active disputes to the dispute-coordinator finished.
	ActiveDisputesReady(JfyiErrorResult<Vec<(SessionIndex, CandidateHash, DisputeStatus)>>),
}

/// The `DisputeSender` keeps track of all ongoing disputes we need to send statements out.
///
/// For each dispute a `SendTask` is responsible for sending to the concerned validators for that
/// particular dispute. The `DisputeSender` keeps track of those tasks, informs them about new
/// sessions/validator sets and cleans them up when they become obsolete.
///
/// The unit of work for the  `DisputeSender` is a dispute, represented by `SendTask`s.
pub struct DisputeSender<M> {
	/// All heads we currently consider active.
	active_heads: Vec<Hash>,

	/// List of currently active sessions.
	///
	/// Value is the hash that was used for the query.
	active_sessions: HashMap<SessionIndex, Hash>,

	/// All ongoing dispute sendings this subsystem is aware of.
	///
	/// Using an `IndexMap` so items can be iterated in the order of insertion.
	disputes: IndexMap<CandidateHash, SendTask<M>>,

	/// Sender to be cloned for `SendTask`s.
	tx: NestingSender<M, DisputeSenderMessage>,

	/// `Some` if we are waiting for a response `DisputeCoordinatorMessage::ActiveDisputes`.
	waiting_for_active_disputes: Option<WaitForActiveDisputesState>,

	/// Future for delaying too frequent creation of dispute sending tasks.
	rate_limit: RateLimit,

	/// Metrics for reporting stats about sent requests.
	metrics: Metrics,
}

/// State we keep while waiting for active disputes.
///
/// When we send `DisputeCoordinatorMessage::ActiveDisputes`, this is the state we keep while
/// waiting for the response.
struct WaitForActiveDisputesState {
	/// Have we seen any new sessions since last refresh?
	have_new_sessions: bool,
}

#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
impl<M: 'static + Send + Sync> DisputeSender<M> {
	/// Create a new `DisputeSender` which can be used to start dispute sendings.
	pub fn new(tx: NestingSender<M, DisputeSenderMessage>, metrics: Metrics) -> Self {
		Self {
			active_heads: Vec::new(),
			active_sessions: HashMap::new(),
			disputes: IndexMap::new(),
			tx,
			waiting_for_active_disputes: None,
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
					NestingSender::new(self.tx.clone(), DisputeSenderMessage::TaskFinish),
					req,
					&self.metrics,
				)
				.await?;
				vacant.insert(send_task);
			},
		}
		Ok(())
	}

	/// Receive message from a background task.
	pub async fn on_message<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		msg: DisputeSenderMessage,
	) -> Result<()> {
		match msg {
			DisputeSenderMessage::TaskFinish(msg) => {
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
						return Ok(())
					},
					Some(task) => task,
				};
				task.on_finished_send(&receiver, result);
			},
			DisputeSenderMessage::ActiveDisputesReady(result) => {
				let state = self.waiting_for_active_disputes.take();
				let have_new_sessions = state.map(|s| s.have_new_sessions).unwrap_or(false);
				let active_disputes = result?;
				self.handle_new_active_disputes(ctx, runtime, active_disputes, have_new_sessions)
					.await?;
			},
		}
		Ok(())
	}

	/// Take care of a change in active leaves.
	///
	/// Update our knowledge on sessions and initiate fetching for new active disputes.
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

		// Not yet waiting for data, request an update:
		match self.waiting_for_active_disputes.take() {
			None => {
				self.waiting_for_active_disputes =
					Some(WaitForActiveDisputesState { have_new_sessions });
				let mut sender = ctx.sender().clone();
				let mut tx = self.tx.clone();

				let get_active_disputes_task = async move {
					let result = get_active_disputes(&mut sender).await;
					let result =
						tx.send_message(DisputeSenderMessage::ActiveDisputesReady(result)).await;
					if let Err(err) = result {
						gum::debug!(
							target: LOG_TARGET,
							?err,
							"Sending `DisputeSenderMessage` from background task failed."
						);
					}
				};

				ctx.spawn("get_active_disputes", Box::pin(get_active_disputes_task))
					.map_err(FatalError::SpawnTask)?;
			},
			Some(state) => {
				let have_new_sessions = state.have_new_sessions || have_new_sessions;
				let new_state = WaitForActiveDisputesState { have_new_sessions };
				self.waiting_for_active_disputes = Some(new_state);
				gum::debug!(
					target: LOG_TARGET,
					"Dispute coordinator slow? We are still waiting for data on next active leaves update."
				);
			},
		}
		Ok(())
	}

	/// Handle new active disputes response.
	///
	/// - Initiate a retry of failed sends which are still active.
	/// - Get new authorities to send messages to.
	/// - Get rid of obsolete tasks and disputes.
	///
	/// This function ensures the `SEND_RATE_LIMIT`, therefore it might block.
	async fn handle_new_active_disputes<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		active_disputes: Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
		have_new_sessions: bool,
	) -> Result<()> {
		let active_disputes: HashSet<_> = active_disputes.into_iter().map(|(_, c, _)| c).collect();

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
		Ok(())
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
		// Cache session info
		if let Err(err) =
			runtime.get_session_info_by_index(ctx.sender(), *head, session_index).await
		{
			gum::debug!(target: LOG_TARGET, ?err, ?session_index, "Can't cache SessionInfo");
		}
		indeces.insert(session_index, *head);
	}
	Ok(indeces)
}

/// Retrieve Set of active disputes from the dispute coordinator.
async fn get_active_disputes<Sender>(
	sender: &mut Sender,
) -> JfyiErrorResult<Vec<(SessionIndex, CandidateHash, DisputeStatus)>>
where
	Sender: SubsystemSender<DisputeCoordinatorMessage>,
{
	let (tx, rx) = oneshot::channel();

	sender.send_message(DisputeCoordinatorMessage::ActiveDisputes(tx)).await;
	rx.await.map_err(|_| JfyiError::AskActiveDisputesCanceled)
}
