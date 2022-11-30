// Copyright 2020 Parity Technologies (UK) Ltd.
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

use std::sync::Arc;

use futures::FutureExt;

use sc_keystore::LocalKeystore;

use polkadot_node_primitives::CandidateVotes;
use polkadot_node_subsystem::{
	overseer, ActivatedLeaf, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::{
	database::Database,
	rolling_session_window::{DatabaseParams, RollingSessionWindow},
};
use polkadot_primitives::v2::{ScrapedOnChainVotes, ValidatorIndex, ValidatorPair};

use crate::{
	error::{FatalResult, JfyiError, Result},
	metrics::Metrics,
	status::{get_active_with_status, SystemClock},
};
use backend::{Backend, OverlayedBackend};
use db::v1::DbBackend;
use fatality::Split;

use self::{
	participation::{ParticipationPriority, ParticipationRequest},
	spam_slots::{SpamSlots, UnconfirmedDisputes},
};

pub(crate) mod backend;
pub(crate) mod db;
pub(crate) mod error;

/// Subsystem after receiving the first active leaf.
mod initialized;
use initialized::Initialized;

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
			.run(ctx, backend, participations, votes, Some(first_leaf), clock)
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
			let db_params =
				DatabaseParams { db: self.store.clone(), db_column: self.config.col_session_data };

			let (first_leaf, rolling_session_window) =
				match get_rolling_session_window(ctx, db_params).await {
					Ok(Some(update)) => update,
					Ok(None) => {
						gum::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
						return Ok(None)
					},
					Err(e) => {
						e.split()?.log();
						continue
					},
				};

			let mut overlay_db = OverlayedBackend::new(&mut backend);
			let (participations, votes, spam_slots, ordering_provider) = match self
				.handle_startup(
					ctx,
					first_leaf.clone(),
					&rolling_session_window,
					&mut overlay_db,
					clock,
				)
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
				Initialized::new(self, rolling_session_window, spam_slots, ordering_provider),
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
		rolling_session_window: &RollingSessionWindow,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		clock: &dyn Clock,
	) -> Result<(
		Vec<(ParticipationPriority, ParticipationRequest)>,
		Vec<ScrapedOnChainVotes>,
		SpamSlots,
		ChainScraper,
	)> {
		// Prune obsolete disputes:
		db::v1::note_earliest_session(overlay_db, rolling_session_window.earliest_session())?;

		let active_disputes = match overlay_db.load_recent_disputes() {
			Ok(Some(disputes)) =>
				get_active_with_status(disputes.into_iter(), clock.now()).collect(),
			Ok(None) => Vec::new(),
			Err(e) => {
				gum::error!(target: LOG_TARGET, "Failed initial load of recent disputes: {:?}", e);
				return Err(e.into())
			},
		};

		let mut participation_requests = Vec::new();
		let mut unconfirmed_disputes: UnconfirmedDisputes = UnconfirmedDisputes::new();
		let (scraper, votes) = ChainScraper::new(ctx.sender(), initial_head).await?;
		for ((session, ref candidate_hash), status) in active_disputes {
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

			let validators = match rolling_session_window.session_info(session) {
				None => {
					gum::warn!(
						target: LOG_TARGET,
						session,
						"Missing info for session which has an active dispute",
					);
					continue
				},
				Some(info) => info.validators.clone(),
			};

			let voted_indices = votes.voted_indices();

			// Determine if there are any missing local statements for this dispute. Validators are
			// filtered if:
			//  1) their statement already exists, or
			//  2) the validator key is not in the local keystore (i.e. the validator is remote).
			// The remaining set only contains local validators that are also missing statements.
			let missing_local_statement = validators
				.iter()
				.enumerate()
				.map(|(index, validator)| (ValidatorIndex(index as _), validator))
				.any(|(index, validator)| {
					!voted_indices.contains(&index) &&
						self.keystore
							.key_pair::<ValidatorPair>(validator)
							.ok()
							.map_or(false, |v| v.is_some())
				});

			let is_included = scraper.is_candidate_included(&votes.candidate_receipt.hash());

			if !status.is_confirmed_concluded() && !is_included {
				unconfirmed_disputes.insert((session, *candidate_hash), voted_indices);
			}

			// Participate for all non-concluded disputes which do not have a
			// recorded local statement.
			if missing_local_statement {
				participation_requests.push((
					ParticipationPriority::with_priority_if(is_included),
					ParticipationRequest::new(votes.candidate_receipt.clone(), session),
				));
			}
		}

		Ok((
			participation_requests,
			votes,
			SpamSlots::recover_from_state(unconfirmed_disputes),
			scraper,
		))
	}
}

/// Wait for `ActiveLeavesUpdate` on startup, returns `None` if `Conclude` signal came first.
#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
async fn get_rolling_session_window<Context>(
	ctx: &mut Context,
	db_params: DatabaseParams,
) -> Result<Option<(ActivatedLeaf, RollingSessionWindow)>> {
	if let Some(leaf) = { wait_for_first_leaf(ctx) }.await? {
		let sender = ctx.sender().clone();
		Ok(Some((
			leaf.clone(),
			RollingSessionWindow::new(sender, leaf.hash, db_params)
				.await
				.map_err(JfyiError::RollingSessionWindow)?,
		)))
	} else {
		Ok(None)
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
