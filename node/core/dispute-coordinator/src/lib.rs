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

//! Implements the dispute coordinator subsystem.
//!
//! This is the central subsystem of the node-side components which participate in disputes.
//! This subsystem wraps a database which tracks all statements observed by all validators over some window of sessions.
//! Votes older than this session window are pruned.
//!
//! This subsystem will be the point which produce dispute votes, either positive or negative, based on locally-observed
//! validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from
//! another node, this will trigger dispute participation to recover and validate the block.

use std::{num::NonZeroUsize, sync::Arc};

use futures::FutureExt;

use gum::CandidateHash;
use sc_keystore::LocalKeystore;

use polkadot_node_primitives::{
	CandidateVotes, DisputeMessage, DisputeMessageCheckError, SignedDisputeStatement,
	DISPUTE_WINDOW,
};
use polkadot_node_subsystem::{
	messages::DisputeDistributionMessage, overseer, ActivatedLeaf, FromOrchestra, OverseerSignal,
	SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::{
	database::Database,
	runtime::{Config as RuntimeInfoConfig, RuntimeInfo},
};
use polkadot_primitives::{
	DisputeStatement, ScrapedOnChainVotes, SessionIndex, SessionInfo, ValidatorIndex,
};

use crate::{
	error::{FatalResult, Result},
	metrics::Metrics,
	status::{get_active_with_status, SystemClock},
};
use backend::{Backend, OverlayedBackend};
use db::v1::DbBackend;
use fatality::Split;

use self::{
	import::{CandidateEnvironment, CandidateVoteState},
	participation::{ParticipationPriority, ParticipationRequest},
	spam_slots::{SpamSlots, UnconfirmedDisputes},
};

pub(crate) mod backend;
pub(crate) mod db;
pub(crate) mod error;

/// Subsystem after receiving the first active leaf.
mod initialized;
use initialized::{InitialData, Initialized};

/// Provider of data scraped from chain.
///
/// If we have seen a candidate included somewhere, we should treat it as priority and will be able
/// to provide an ordering for participation. Thus a dispute for a candidate where we can get some
/// ordering is high-priority (we know it is a valid dispute) and those can be ordered by
/// `participation` based on `relay_parent` block number and other metrics, so each validator will
/// participate in disputes in a similar order, which ensures we will be resolving disputes, even
/// under heavy load.
mod scraping;
use scraping::ChainScraper;

/// When importing votes we will check via the `ordering` module, whether or not we know of the
/// candidate to be included somewhere. If not, the votes might be spam, in this case we want to
/// limit the amount of locally imported votes, to prevent DoS attacks/resource exhaustion. The
/// `spam_slots` module helps keeping track of unconfirmed disputes per validators, if a spam slot
/// gets full, we will drop any further potential spam votes from that validator and report back
/// that the import failed. Which will lead to any honest validator to retry, thus the spam slots
/// can be relatively small, as a drop is not fatal.
mod spam_slots;

/// Handling of participation requests via `Participation`.
///
/// `Participation` provides an API (`Participation::queue_participation`) for queuing of dispute participations and will process those
/// participation requests, such that most important/urgent disputes will be resolved and processed
/// first and more importantly it will order requests in a way so disputes will get resolved, even
/// if there are lots of them.
pub(crate) mod participation;

/// Pure processing of vote imports.
pub(crate) mod import;

/// Metrics types.
mod metrics;

/// Status tracking of disputes (`DisputeStatus`).
mod status;

use crate::status::Clock;

#[cfg(test)]
mod tests;

pub(crate) const LOG_TARGET: &str = "parachain::dispute-coordinator";

/// An implementation of the dispute coordinator subsystem.
pub struct DisputeCoordinatorSubsystem {
	config: Config,
	store: Arc<dyn Database>,
	keystore: Arc<LocalKeystore>,
	metrics: Metrics,
}

/// Configuration for the dispute coordinator subsystem.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The data column in the store to use for dispute data.
	pub col_dispute_data: u32,
	/// The data column in the store to use for session data.
	pub col_session_data: u32,
}

impl Config {
	fn column_config(&self) -> db::v1::ColumnConfiguration {
		db::v1::ColumnConfiguration {
			col_dispute_data: self.col_dispute_data,
			col_session_data: self.col_session_data,
		}
	}
}

#[overseer::subsystem(DisputeCoordinator, error=SubsystemError, prefix=self::overseer)]
impl<Context: Send> DisputeCoordinatorSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = async {
			let backend = DbBackend::new(
				self.store.clone(),
				self.config.column_config(),
				self.metrics.clone(),
			);
			self.run(ctx, backend, Box::new(SystemClock))
				.await
				.map_err(|e| SubsystemError::with_origin("dispute-coordinator", e))
		}
		.boxed();

		SpawnedSubsystem { name: "dispute-coordinator-subsystem", future }
	}
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
impl DisputeCoordinatorSubsystem {
	/// Create a new instance of the subsystem.
	pub fn new(
		store: Arc<dyn Database>,
		config: Config,
		keystore: Arc<LocalKeystore>,
		metrics: Metrics,
	) -> Self {
		Self { store, config, keystore, metrics }
	}

	/// Initialize and afterwards run `Initialized::run`.
	async fn run<B, Context>(
		self,
		mut ctx: Context,
		backend: B,
		clock: Box<dyn Clock>,
	) -> FatalResult<()>
	where
		B: Backend + 'static,
	{
		let res = self.initialize(&mut ctx, backend, &*clock).await?;

		let (participations, votes, first_leaf, initialized, backend) = match res {
			// Concluded:
			None => return Ok(()),
			Some(r) => r,
		};

		initialized
			.run(ctx, backend, Some(InitialData { participations, votes, leaf: first_leaf }), clock)
			.await
	}

	/// Make sure to recover participations properly on startup.
	async fn initialize<B, Context>(
		self,
		ctx: &mut Context,
		mut backend: B,
		clock: &(dyn Clock),
	) -> FatalResult<
		Option<(
			Vec<(ParticipationPriority, ParticipationRequest)>,
			Vec<ScrapedOnChainVotes>,
			ActivatedLeaf,
			Initialized,
			B,
		)>,
	>
	where
		B: Backend + 'static,
	{
		loop {
			let first_leaf = match wait_for_first_leaf(ctx).await {
				Ok(Some(activated_leaf)) => activated_leaf,
				Ok(None) => continue,
				Err(e) => {
					e.split()?.log();
					continue
				},
			};

			// `RuntimeInfo` cache should match `DISPUTE_WINDOW` so that we can
			// keep all sessions for a dispute window
			let mut runtime_info = RuntimeInfo::new_with_config(RuntimeInfoConfig {
				keystore: None,
				session_cache_lru_size: NonZeroUsize::new(DISPUTE_WINDOW.get() as usize)
					.expect("DISPUTE_WINDOW can't be 0; qed."),
			});
			let mut overlay_db = OverlayedBackend::new(&mut backend);
			let (
				participations,
				votes,
				spam_slots,
				ordering_provider,
				highest_session_seen,
				gaps_in_cache,
			) = match self
				.handle_startup(ctx, first_leaf.clone(), &mut runtime_info, &mut overlay_db, clock)
				.await
			{
				Ok(v) => v,
				Err(e) => {
					e.split()?.log();
					continue
				},
			};
			if !overlay_db.is_empty() {
				let ops = overlay_db.into_write_ops();
				backend.write(ops)?;
			}

			return Ok(Some((
				participations,
				votes,
				first_leaf,
				Initialized::new(
					self,
					runtime_info,
					spam_slots,
					ordering_provider,
					highest_session_seen,
					gaps_in_cache,
				),
				backend,
			)))
		}
	}

	// Restores the subsystem's state before proceeding with the main event loop.
	//
	// - Prune any old disputes.
	// - Find disputes we need to participate in.
	// - Initialize spam slots & OrderingProvider.
	async fn handle_startup<Context>(
		&self,
		ctx: &mut Context,
		initial_head: ActivatedLeaf,
		runtime_info: &mut RuntimeInfo,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		clock: &dyn Clock,
	) -> Result<(
		Vec<(ParticipationPriority, ParticipationRequest)>,
		Vec<ScrapedOnChainVotes>,
		SpamSlots,
		ChainScraper,
		SessionIndex,
		bool,
	)> {
		let now = clock.now();

		let active_disputes = match overlay_db.load_recent_disputes() {
			Ok(disputes) => disputes
				.map(|disputes| get_active_with_status(disputes.into_iter(), now))
				.into_iter()
				.flatten(),
			Err(e) => {
				gum::error!(target: LOG_TARGET, "Failed initial load of recent disputes: {:?}", e);
				return Err(e.into())
			},
		};

		// We assume the highest session is the passed leaf. If we can't get the session index
		// we can't initialize the subsystem so we'll wait for a new leaf
		let highest_session = runtime_info
			.get_session_index_for_child(ctx.sender(), initial_head.hash)
			.await?;

		let mut gap_in_cache = false;
		// Cache the sessions. A failure to fetch a session here is not that critical so we
		// won't abort the initialization
		for idx in highest_session.saturating_sub(DISPUTE_WINDOW.get() - 1)..=highest_session {
			if let Err(e) = runtime_info
				.get_session_info_by_index(ctx.sender(), initial_head.hash, idx)
				.await
			{
				gum::debug!(
					target: LOG_TARGET,
					leaf_hash = ?initial_head.hash,
					session_idx = idx,
					err = ?e,
					"Can't cache SessionInfo during subsystem initialization. Skipping session."
				);
				gap_in_cache = true;
				continue
			};
		}

		// Prune obsolete disputes:
		db::v1::note_earliest_session(
			overlay_db,
			highest_session.saturating_sub(DISPUTE_WINDOW.get() - 1),
		)?;

		let mut participation_requests = Vec::new();
		let mut spam_disputes: UnconfirmedDisputes = UnconfirmedDisputes::new();
		let leaf_hash = initial_head.hash;
		let (scraper, votes) = ChainScraper::new(ctx.sender(), initial_head).await?;
		for ((session, ref candidate_hash), _) in active_disputes {
			let env = match CandidateEnvironment::new(
				&self.keystore,
				ctx,
				runtime_info,
				highest_session,
				leaf_hash,
			)
			.await
			{
				None => {
					gum::warn!(
						target: LOG_TARGET,
						session,
						"We are lacking a `SessionInfo` for handling db votes on startup."
					);

					continue
				},
				Some(env) => env,
			};

			let votes: CandidateVotes =
				match overlay_db.load_candidate_votes(session, candidate_hash) {
					Ok(Some(votes)) => votes.into(),
					Ok(None) => continue,
					Err(e) => {
						gum::error!(
							target: LOG_TARGET,
							"Failed initial load of candidate votes: {:?}",
							e
						);
						continue
					},
				};
			let vote_state = CandidateVoteState::new(votes, &env, now);

			let potential_spam = is_potential_spam(&scraper, &vote_state, candidate_hash);
			let is_included =
				scraper.is_candidate_included(&vote_state.votes().candidate_receipt.hash());

			if potential_spam {
				gum::trace!(
					target: LOG_TARGET,
					?session,
					?candidate_hash,
					"Found potential spam dispute on startup"
				);
				spam_disputes
					.insert((session, *candidate_hash), vote_state.votes().voted_indices());
			} else {
				// Participate if need be:
				if vote_state.own_vote_missing() {
					gum::trace!(
						target: LOG_TARGET,
						?session,
						?candidate_hash,
						"Found valid dispute, with no vote from us on startup - participating."
					);
					let request_timer = self.metrics.time_participation_pipeline();
					participation_requests.push((
						ParticipationPriority::with_priority_if(is_included),
						ParticipationRequest::new(
							vote_state.votes().candidate_receipt.clone(),
							session,
							request_timer,
						),
					));
				}
				// Else make sure our own vote is distributed:
				else {
					gum::trace!(
						target: LOG_TARGET,
						?session,
						?candidate_hash,
						"Found valid dispute, with vote from us on startup - send vote."
					);
					send_dispute_messages(ctx, &env, &vote_state).await;
				}
			}
		}

		Ok((
			participation_requests,
			votes,
			SpamSlots::recover_from_state(spam_disputes),
			scraper,
			highest_session,
			gap_in_cache,
		))
	}
}

/// Wait for `ActiveLeavesUpdate`, returns `None` if `Conclude` signal came first.
#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
async fn wait_for_first_leaf<Context>(ctx: &mut Context) -> Result<Option<ActivatedLeaf>> {
	loop {
		match ctx.recv().await? {
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(None),
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
				if let Some(activated) = update.activated {
					return Ok(Some(activated))
				}
			},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(_, _)) => {},
			FromOrchestra::Communication { msg } =>
			// NOTE: We could technically actually handle a couple of message types, even if
			// not initialized (e.g. all requests that only query the database). The problem
			// is, we would deliver potentially outdated information, especially in the event
			// of bugs where initialization fails for a while (e.g. `SessionInfo`s are not
			// available). So instead of telling subsystems, everything is fine, because of an
			// hour old database state, we should rather cancel contained oneshots and delay
			// finality until we are fully functional.
			{
				gum::warn!(
					target: LOG_TARGET,
					?msg,
					"Received msg before first active leaves update. This is not expected - message will be dropped."
				)
			},
		}
	}
}

/// Check wheter a dispute for the given candidate could be spam.
///
/// That is the candidate could be made up.
pub fn is_potential_spam<V>(
	scraper: &ChainScraper,
	vote_state: &CandidateVoteState<V>,
	candidate_hash: &CandidateHash,
) -> bool {
	let is_disputed = vote_state.is_disputed();
	let is_included = scraper.is_candidate_included(candidate_hash);
	let is_backed = scraper.is_candidate_backed(candidate_hash);
	let is_confirmed = vote_state.is_confirmed();

	is_disputed && !is_included && !is_backed && !is_confirmed
}

/// Tell dispute-distribution to send all our votes.
///
/// Should be called on startup for all active disputes where there are votes from us already.
#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
async fn send_dispute_messages<Context>(
	ctx: &mut Context,
	env: &CandidateEnvironment<'_>,
	vote_state: &CandidateVoteState<CandidateVotes>,
) {
	for own_vote in vote_state.own_votes().into_iter().flatten() {
		let (validator_index, (kind, sig)) = own_vote;
		let public_key = if let Some(key) = env.session_info().validators.get(*validator_index) {
			key.clone()
		} else {
			gum::error!(
				target: LOG_TARGET,
				?validator_index,
				session_index = ?env.session_index(),
				"Could not find our own key in `SessionInfo`"
			);
			continue
		};
		let our_vote_signed = SignedDisputeStatement::new_checked(
			kind.clone(),
			vote_state.votes().candidate_receipt.hash(),
			env.session_index(),
			public_key,
			sig.clone(),
		);
		let our_vote_signed = match our_vote_signed {
			Ok(signed) => signed,
			Err(()) => {
				gum::error!(
					target: LOG_TARGET,
					"Checking our own signature failed - db corruption?"
				);
				continue
			},
		};
		let dispute_message = match make_dispute_message(
			env.session_info(),
			vote_state.votes(),
			our_vote_signed,
			*validator_index,
		) {
			Err(err) => {
				gum::debug!(target: LOG_TARGET, ?err, "Creating dispute message failed.");
				continue
			},
			Ok(dispute_message) => dispute_message,
		};

		ctx.send_message(DisputeDistributionMessage::SendDispute(dispute_message)).await;
	}
}

#[derive(Debug, thiserror::Error)]
pub enum DisputeMessageCreationError {
	#[error("There was no opposite vote available")]
	NoOppositeVote,
	#[error("Found vote had an invalid validator index that could not be found")]
	InvalidValidatorIndex,
	#[error("Statement found in votes had invalid signature.")]
	InvalidStoredStatement,
	#[error(transparent)]
	InvalidStatementCombination(DisputeMessageCheckError),
}

/// Create a `DisputeMessage` to be sent to `DisputeDistribution`.
pub fn make_dispute_message(
	info: &SessionInfo,
	votes: &CandidateVotes,
	our_vote: SignedDisputeStatement,
	our_index: ValidatorIndex,
) -> std::result::Result<DisputeMessage, DisputeMessageCreationError> {
	let validators = &info.validators;

	let (valid_statement, valid_index, invalid_statement, invalid_index) =
		if let DisputeStatement::Valid(_) = our_vote.statement() {
			let (validator_index, (statement_kind, validator_signature)) =
				votes.invalid.iter().next().ok_or(DisputeMessageCreationError::NoOppositeVote)?;
			let other_vote = SignedDisputeStatement::new_checked(
				DisputeStatement::Invalid(*statement_kind),
				*our_vote.candidate_hash(),
				our_vote.session_index(),
				validators
					.get(*validator_index)
					.ok_or(DisputeMessageCreationError::InvalidValidatorIndex)?
					.clone(),
				validator_signature.clone(),
			)
			.map_err(|()| DisputeMessageCreationError::InvalidStoredStatement)?;
			(our_vote, our_index, other_vote, *validator_index)
		} else {
			let (validator_index, (statement_kind, validator_signature)) = votes
				.valid
				.raw()
				.iter()
				.next()
				.ok_or(DisputeMessageCreationError::NoOppositeVote)?;
			let other_vote = SignedDisputeStatement::new_checked(
				DisputeStatement::Valid(*statement_kind),
				*our_vote.candidate_hash(),
				our_vote.session_index(),
				validators
					.get(*validator_index)
					.ok_or(DisputeMessageCreationError::InvalidValidatorIndex)?
					.clone(),
				validator_signature.clone(),
			)
			.map_err(|()| DisputeMessageCreationError::InvalidStoredStatement)?;
			(other_vote, *validator_index, our_vote, our_index)
		};

	DisputeMessage::from_signed_statements(
		valid_statement,
		valid_index,
		invalid_statement,
		invalid_index,
		votes.candidate_receipt.clone(),
		info,
	)
	.map_err(DisputeMessageCreationError::InvalidStatementCombination)
}
