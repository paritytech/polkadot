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

//! Dispute coordinator subsystem in initialized state (after first active leaf is received).

use std::{
	collections::{BTreeMap, HashSet},
	sync::Arc,
};

use futures::{channel::mpsc, FutureExt, StreamExt};

use sc_keystore::LocalKeystore;

use polkadot_node_primitives::{
	CandidateVotes, DisputeMessage, DisputeMessageCheckError, SignedDisputeStatement,
	DISPUTE_WINDOW,
};
use polkadot_node_subsystem::{
	messages::{
		BlockDescription, DisputeCoordinatorMessage, DisputeDistributionMessage,
		ImportStatementsResult,
	},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, OverseerSignal,
};
use polkadot_node_subsystem_util::rolling_session_window::{
	RollingSessionWindow, SessionWindowUpdate, SessionsUnavailable,
};
use polkadot_primitives::v2::{
	byzantine_threshold, BlockNumber, CandidateHash, CandidateReceipt, CompactStatement,
	DisputeStatement, DisputeStatementSet, Hash, ScrapedOnChainVotes, SessionIndex, SessionInfo,
	ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorPair, ValidatorSignature,
};

use crate::{
	error::{log_error, Error, FatalError, FatalResult, JfyiError, JfyiResult, Result},
	metrics::Metrics,
	status::{get_active_with_status, Clock, DisputeStatus, Timestamp},
	DisputeCoordinatorSubsystem, LOG_TARGET,
};

use super::{
	backend::Backend,
	db,
	participation::{
		self, Participation, ParticipationPriority, ParticipationRequest, ParticipationStatement,
		WorkerMessageReceiver,
	},
	scraping::ChainScraper,
	spam_slots::SpamSlots,
	OverlayedBackend,
};

/// After the first active leaves update we transition to `Initialized` state.
///
/// Before the first active leaves update we can't really do much. We cannot check incoming
/// statements for validity, we cannot query orderings, we have no valid `RollingSessionWindow`,
/// ...
pub struct Initialized {
	keystore: Arc<LocalKeystore>,
	rolling_session_window: RollingSessionWindow,
	highest_session: SessionIndex,
	spam_slots: SpamSlots,
	participation: Participation,
	scraper: ChainScraper,
	participation_receiver: WorkerMessageReceiver,
	metrics: Metrics,
	// This tracks only rolling session window failures.
	// It can be a `Vec` if the need to track more arises.
	error: Option<SessionsUnavailable>,
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
impl Initialized {
	/// Make initialized subsystem, ready to `run`.
	pub fn new(
		subsystem: DisputeCoordinatorSubsystem,
		rolling_session_window: RollingSessionWindow,
		spam_slots: SpamSlots,
		scraper: ChainScraper,
	) -> Self {
		let DisputeCoordinatorSubsystem { config: _, store: _, keystore, metrics } = subsystem;

		let (participation_sender, participation_receiver) = mpsc::channel(1);
		let participation = Participation::new(participation_sender);
		let highest_session = rolling_session_window.latest_session();

		Self {
			keystore,
			rolling_session_window,
			highest_session,
			spam_slots,
			scraper,
			participation,
			participation_receiver,
			metrics,
			error: None,
		}
	}

	/// Run the initialized subsystem.
	///
	/// Optionally supply initial participations and a first leaf to process.
	pub async fn run<B, Context>(
		mut self,
		mut ctx: Context,
		mut backend: B,
		mut participations: Vec<(ParticipationPriority, ParticipationRequest)>,
		mut votes: Vec<ScrapedOnChainVotes>,
		mut first_leaf: Option<ActivatedLeaf>,
		clock: Box<dyn Clock>,
	) -> FatalResult<()>
	where
		B: Backend,
	{
		loop {
			let res = self
				.run_until_error(
					&mut ctx,
					&mut backend,
					&mut participations,
					&mut votes,
					&mut first_leaf,
					&*clock,
				)
				.await;
			if let Ok(()) = res {
				gum::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				return Ok(())
			}
			log_error(res)?;
		}
	}

	// Run the subsystem until an error is encountered or a `conclude` signal is received.
	// Most errors are non-fatal and should lead to another call to this function.
	//
	// A return value of `Ok` indicates that an exit should be made, while non-fatal errors
	// lead to another call to this function.
	async fn run_until_error<B, Context>(
		&mut self,
		ctx: &mut Context,
		backend: &mut B,
		participations: &mut Vec<(ParticipationPriority, ParticipationRequest)>,
		on_chain_votes: &mut Vec<ScrapedOnChainVotes>,
		first_leaf: &mut Option<ActivatedLeaf>,
		clock: &dyn Clock,
	) -> Result<()>
	where
		B: Backend,
	{
		for (priority, request) in participations.drain(..) {
			self.participation.queue_participation(ctx, priority, request).await?;
		}

		{
			let mut overlay_db = OverlayedBackend::new(backend);
			for votes in on_chain_votes.drain(..) {
				let _ = self
					.process_on_chain_votes(ctx, &mut overlay_db, votes, clock.now())
					.await
					.map_err(|error| {
						gum::warn!(
							target: LOG_TARGET,
							?error,
							"Skipping scraping block due to error",
						);
					});
			}
			if !overlay_db.is_empty() {
				let ops = overlay_db.into_write_ops();
				backend.write(ops)?;
			}
		}

		if let Some(first_leaf) = first_leaf.take() {
			// Also provide first leaf to participation for good measure.
			self.participation
				.process_active_leaves_update(ctx, &ActiveLeavesUpdate::start_work(first_leaf))
				.await?;
		}

		loop {
			let mut overlay_db = OverlayedBackend::new(backend);
			let default_confirm = Box::new(|| Ok(()));
			let confirm_write =
				match MuxedMessage::receive(ctx, &mut self.participation_receiver).await? {
					MuxedMessage::Participation(msg) => {
						let ParticipationStatement {
							session,
							candidate_hash,
							candidate_receipt,
							outcome,
						} = self.participation.get_participation_result(ctx, msg).await?;
						if let Some(valid) = outcome.validity() {
							self.issue_local_statement(
								ctx,
								&mut overlay_db,
								candidate_hash,
								candidate_receipt,
								session,
								valid,
								clock.now(),
							)
							.await?;
						}
						default_confirm
					},
					MuxedMessage::Subsystem(msg) => match msg {
						FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
						FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
							self.process_active_leaves_update(
								ctx,
								&mut overlay_db,
								update,
								clock.now(),
							)
							.await?;
							default_confirm
						},
						FromOrchestra::Signal(OverseerSignal::BlockFinalized(_, n)) => {
							self.scraper.process_finalized_block(&n);
							default_confirm
						},
						FromOrchestra::Communication { msg } =>
							self.handle_incoming(ctx, &mut overlay_db, msg, clock.now()).await?,
					},
				};

			if !overlay_db.is_empty() {
				let ops = overlay_db.into_write_ops();
				backend.write(ops)?;
			}
			// even if the changeset was empty,
			// otherwise the caller will error.
			confirm_write()?;
		}
	}

	async fn process_active_leaves_update<Context>(
		&mut self,
		ctx: &mut Context,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		update: ActiveLeavesUpdate,
		now: u64,
	) -> Result<()> {
		let on_chain_votes =
			self.scraper.process_active_leaves_update(ctx.sender(), &update).await?;
		self.participation.process_active_leaves_update(ctx, &update).await?;

		if let Some(new_leaf) = update.activated {
			match self
				.rolling_session_window
				.cache_session_info_for_head(ctx.sender(), new_leaf.hash)
				.await
			{
				Err(e) => {
					gum::warn!(
						target: LOG_TARGET,
						err = ?e,
						"Failed to update session cache for disputes",
					);
					self.error = Some(e);
				},
				Ok(SessionWindowUpdate::Advanced {
					new_window_end: window_end,
					new_window_start,
					..
				}) => {
					self.error = None;
					let session = window_end;
					if self.highest_session < session {
						gum::trace!(target: LOG_TARGET, session, "Observed new session. Pruning");

						self.highest_session = session;

						db::v1::note_current_session(overlay_db, session)?;
						self.spam_slots.prune_old(new_window_start);
					}
				},
				Ok(SessionWindowUpdate::Unchanged) => {},
			};

			// The `runtime-api` subsystem has an internal queue which serializes the execution,
			// so there is no point in running these in parallel.
			for votes in on_chain_votes {
				let _ = self.process_on_chain_votes(ctx, overlay_db, votes, now).await.map_err(
					|error| {
						gum::warn!(
							target: LOG_TARGET,
							?error,
							"Skipping scraping block due to error",
						);
					},
				);
			}
		}

		Ok(())
	}

	/// Scrapes on-chain votes (backing votes and concluded disputes) for a active leaf of the
	/// relay chain.
	async fn process_on_chain_votes<Context>(
		&mut self,
		ctx: &mut Context,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		votes: ScrapedOnChainVotes,
		now: u64,
	) -> Result<()> {
		let ScrapedOnChainVotes { session, backing_validators_per_candidate, disputes } = votes;

		if backing_validators_per_candidate.is_empty() && disputes.is_empty() {
			return Ok(())
		}

		// Obtain the session info, for sake of `ValidatorId`s
		// either from the rolling session window.
		// Must be called _after_ `fn cache_session_info_for_head`
		// which guarantees that the session info is available
		// for the current session.
		let session_info: SessionInfo =
			if let Some(session_info) = self.rolling_session_window.session_info(session) {
				session_info.clone()
			} else {
				gum::warn!(
					target: LOG_TARGET,
					?session,
					"Could not retrieve session info from rolling session window",
				);
				return Ok(())
			};

		// Scraped on-chain backing votes for the candidates with
		// the new active leaf as if we received them via gossip.
		for (candidate_receipt, backers) in backing_validators_per_candidate {
			let relay_parent = candidate_receipt.descriptor.relay_parent;
			let candidate_hash = candidate_receipt.hash();
			let statements = backers
				.into_iter()
				.filter_map(|(validator_index, attestation)| {
					let validator_public: ValidatorId = session_info
						.validators
						.get(validator_index.0 as usize)
						.or_else(|| {
							gum::error!(
								target: LOG_TARGET,
								?session,
								?validator_index,
								"Missing public key for validator",
							);
							None
						})
						.cloned()?;
					let validator_signature = attestation.signature().clone();
					let valid_statement_kind =
						match attestation.to_compact_statement(candidate_hash) {
							CompactStatement::Seconded(_) =>
								ValidDisputeStatementKind::BackingSeconded(relay_parent),
							CompactStatement::Valid(_) =>
								ValidDisputeStatementKind::BackingValid(relay_parent),
						};
					let signed_dispute_statement =
						SignedDisputeStatement::new_unchecked_from_trusted_source(
							DisputeStatement::Valid(valid_statement_kind),
							candidate_hash,
							session,
							validator_public,
							validator_signature,
						);
					Some((signed_dispute_statement, validator_index))
				})
				.collect();

			let import_result = self
				.handle_import_statements(
					ctx,
					overlay_db,
					candidate_hash,
					MaybeCandidateReceipt::Provides(candidate_receipt),
					session,
					statements,
					now,
				)
				.await?;
			match import_result {
				ImportStatementsResult::ValidImport => gum::trace!(
					target: LOG_TARGET,
					?relay_parent,
					?session,
					"Imported backing votes from chain"
				),
				ImportStatementsResult::InvalidImport => gum::warn!(
					target: LOG_TARGET,
					?relay_parent,
					?session,
					"Attempted import of on-chain backing votes failed"
				),
			}
		}

		// Import concluded disputes from on-chain, this already went through a vote so it's assumed
		// as verified. This will only be stored, gossiping it is not necessary.

		// First try to obtain all the backings which ultimately contain the candidate
		// receipt which we need.

		for DisputeStatementSet { candidate_hash, session, statements } in disputes {
			let statements = statements
				.into_iter()
				.filter_map(|(dispute_statement, validator_index, validator_signature)| {
					let session_info: SessionInfo = if let Some(session_info) =
						self.rolling_session_window.session_info(session)
					{
						session_info.clone()
					} else {
						gum::warn!(
								target: LOG_TARGET,
								?candidate_hash,
								?session,
								"Could not retrieve session info from rolling session window for recently concluded dispute");
						return None
					};

					let validator_public: ValidatorId = session_info
						.validators
						.get(validator_index.0 as usize)
						.or_else(|| {
							gum::error!(
								target: LOG_TARGET,
								?candidate_hash,
								?session,
								"Missing public key for validator {:?} that participated in concluded dispute",
								&validator_index);
							None
						})
						.cloned()?;

					Some((
						SignedDisputeStatement::new_unchecked_from_trusted_source(
							dispute_statement,
							candidate_hash,
							session,
							validator_public,
							validator_signature,
						),
						validator_index,
					))
				})
				.collect::<Vec<_>>();
			let import_result = self
				.handle_import_statements(
					ctx,
					overlay_db,
					candidate_hash,
					// TODO <https://github.com/paritytech/polkadot/issues/4011>
					MaybeCandidateReceipt::AssumeBackingVotePresent,
					session,
					statements,
					now,
				)
				.await?;
			match import_result {
				ImportStatementsResult::ValidImport => gum::trace!(
					target: LOG_TARGET,
					?candidate_hash,
					?session,
					"Imported statement of concluded dispute from on-chain"
				),
				ImportStatementsResult::InvalidImport => gum::warn!(
					target: LOG_TARGET,
					?candidate_hash,
					?session,
					"Attempted import of on-chain statement of concluded dispute failed"
				),
			}
		}

		Ok(())
	}

	async fn handle_incoming<Context>(
		&mut self,
		ctx: &mut Context,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		message: DisputeCoordinatorMessage,
		now: Timestamp,
	) -> Result<Box<dyn FnOnce() -> JfyiResult<()>>> {
		match message {
			DisputeCoordinatorMessage::ImportStatements {
				candidate_hash,
				candidate_receipt,
				session,
				statements,
				pending_confirmation,
			} => {
				let outcome = self
					.handle_import_statements(
						ctx,
						overlay_db,
						candidate_hash,
						MaybeCandidateReceipt::Provides(candidate_receipt),
						session,
						statements,
						now,
					)
					.await?;
				let report = move || match pending_confirmation {
					Some(pending_confirmation) => pending_confirmation
						.send(outcome)
						.map_err(|_| JfyiError::DisputeImportOneshotSend),
					None => Ok(()),
				};

				match outcome {
					ImportStatementsResult::InvalidImport => {
						report()?;
					},
					// In case of valid import, delay confirmation until actual disk write:
					ImportStatementsResult::ValidImport => return Ok(Box::new(report)),
				}
			},
			DisputeCoordinatorMessage::RecentDisputes(tx) => {
				// Return error if session information is missing.
				self.ensure_available_session_info()?;

				let recent_disputes = if let Some(disputes) = overlay_db.load_recent_disputes()? {
					disputes
				} else {
					BTreeMap::new()
				};

				let _ = tx.send(recent_disputes.keys().cloned().collect());
			},
			DisputeCoordinatorMessage::ActiveDisputes(tx) => {
				// Return error if session information is missing.
				self.ensure_available_session_info()?;

				let recent_disputes = if let Some(disputes) = overlay_db.load_recent_disputes()? {
					disputes
				} else {
					BTreeMap::new()
				};

				let _ = tx.send(
					get_active_with_status(recent_disputes.into_iter(), now)
						.map(|(k, _)| k)
						.collect(),
				);
			},
			DisputeCoordinatorMessage::QueryCandidateVotes(query, tx) => {
				// Return error if session information is missing.
				self.ensure_available_session_info()?;

				let mut query_output = Vec::new();
				for (session_index, candidate_hash) in query {
					if let Some(v) =
						overlay_db.load_candidate_votes(session_index, &candidate_hash)?
					{
						query_output.push((session_index, candidate_hash, v.into()));
					} else {
						gum::debug!(
							target: LOG_TARGET,
							session_index,
							"No votes found for candidate",
						);
					}
				}
				let _ = tx.send(query_output);
			},
			DisputeCoordinatorMessage::IssueLocalStatement(
				session,
				candidate_hash,
				candidate_receipt,
				valid,
			) => {
				self.issue_local_statement(
					ctx,
					overlay_db,
					candidate_hash,
					candidate_receipt,
					session,
					valid,
					now,
				)
				.await?;
			},
			DisputeCoordinatorMessage::DetermineUndisputedChain {
				base: (base_number, base_hash),
				block_descriptions,
				tx,
			} => {
				// Return error if session information is missing.
				self.ensure_available_session_info()?;

				let undisputed_chain = determine_undisputed_chain(
					overlay_db,
					base_number,
					base_hash,
					block_descriptions,
				)?;

				let _ = tx.send(undisputed_chain);
			},
		}

		Ok(Box::new(|| Ok(())))
	}

	// Helper function for checking subsystem errors in message processing.
	fn ensure_available_session_info(&self) -> Result<()> {
		if let Some(subsystem_error) = self.error.clone() {
			return Err(Error::RollingSessionWindow(subsystem_error))
		}

		Ok(())
	}

	async fn handle_import_statements<Context>(
		&mut self,
		ctx: &mut Context,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		candidate_hash: CandidateHash,
		candidate_receipt: MaybeCandidateReceipt,
		session: SessionIndex,
		statements: Vec<(SignedDisputeStatement, ValidatorIndex)>,
		now: Timestamp,
	) -> Result<ImportStatementsResult> {
		if session + DISPUTE_WINDOW.get() < self.highest_session {
			// It is not valid to participate in an ancient dispute (spam?).
			return Ok(ImportStatementsResult::InvalidImport)
		}

		let session_info = match self.rolling_session_window.session_info(session) {
			None => {
				gum::warn!(
					target: LOG_TARGET,
					session,
					"Importing statement lacks info for session which has an active dispute",
				);

				return Ok(ImportStatementsResult::InvalidImport)
			},
			Some(info) => info,
		};
		let validators = session_info.validators.clone();

		let n_validators = validators.len();

		let supermajority_threshold =
			polkadot_primitives::v2::supermajority_threshold(n_validators);

		// In case we are not provided with a candidate receipt
		// we operate under the assumption, that a previous vote
		// which included a `CandidateReceipt` was seen.
		// This holds since every block is preceeded by the `Backing`-phase.
		//
		// There is one exception: A sufficiently sophisticated attacker could prevent
		// us from seeing the backing votes by witholding arbitrary blocks, and hence we do
		// not have a `CandidateReceipt` available.
		let (mut votes, mut votes_changed) = match overlay_db
			.load_candidate_votes(session, &candidate_hash)?
			.map(CandidateVotes::from)
		{
			Some(votes) => (votes, false),
			None =>
				if let MaybeCandidateReceipt::Provides(candidate_receipt) = candidate_receipt {
					(
						CandidateVotes {
							candidate_receipt,
							valid: Vec::new(),
							invalid: Vec::new(),
						},
						true,
					)
				} else {
					gum::warn!(
						target: LOG_TARGET,
						session,
						"Not seen backing vote for candidate which has an active dispute",
					);
					return Ok(ImportStatementsResult::InvalidImport)
				},
		};
		let candidate_receipt = votes.candidate_receipt.clone();
		let was_concluded_valid = votes.valid.len() >= supermajority_threshold;
		let was_concluded_invalid = votes.invalid.len() >= supermajority_threshold;

		let mut recent_disputes = overlay_db.load_recent_disputes()?.unwrap_or_default();
		let controlled_indices = find_controlled_validator_indices(&self.keystore, &validators);

		// Whether we already cast a vote in that dispute:
		let voted_already = {
			let mut our_votes = votes.voted_indices();
			our_votes.retain(|index| controlled_indices.contains(index));
			!our_votes.is_empty()
		};

		let was_confirmed = recent_disputes
			.get(&(session, candidate_hash))
			.map_or(false, |s| s.is_confirmed_concluded());

		let is_included = self.scraper.is_candidate_included(&candidate_receipt.hash());

		let is_local = statements
			.iter()
			.find(|(_, index)| controlled_indices.contains(index))
			.is_some();

		// Indexes of the validators issued 'invalid' statements. Will be used to populate spam slots.
		let mut fresh_invalid_statement_issuers = Vec::new();

		// Update candidate votes.
		for (statement, val_index) in &statements {
			if validators
				.get(val_index.0 as usize)
				.map_or(true, |v| v != statement.validator_public())
			{
				gum::debug!(
				target: LOG_TARGET,
				?val_index,
				session,
				claimed_key = ?statement.validator_public(),
				"Validator index doesn't match claimed key",
				);

				continue
			}

			match statement.statement() {
				DisputeStatement::Valid(valid_kind) => {
					let fresh = insert_into_statement_vec(
						&mut votes.valid,
						*valid_kind,
						*val_index,
						statement.validator_signature().clone(),
					);

					if !fresh {
						continue
					}

					votes_changed = true;
					self.metrics.on_valid_vote();
				},
				DisputeStatement::Invalid(invalid_kind) => {
					let fresh = insert_into_statement_vec(
						&mut votes.invalid,
						*invalid_kind,
						*val_index,
						statement.validator_signature().clone(),
					);

					if !fresh {
						continue
					}

					fresh_invalid_statement_issuers.push(*val_index);
					votes_changed = true;
					self.metrics.on_invalid_vote();
				},
			}
		}

		// Whether or not we know already that this is a good dispute:
		//
		// Note we can only know for sure whether we reached the `byzantine_threshold`  after
		// updating candidate votes above, therefore the spam checking is afterwards:
		let is_confirmed = is_included ||
			was_confirmed ||
			is_local || votes.voted_indices().len() >
			byzantine_threshold(n_validators);

		// Potential spam:
		if !is_confirmed && !fresh_invalid_statement_issuers.is_empty() {
			let mut free_spam_slots_available = true;
			// Only allow import if all validators voting invalid, have not exceeded
			// their spam slots:
			for index in fresh_invalid_statement_issuers {
				// Disputes can only be triggered via an invalidity stating vote, thus we only
				// need to increase spam slots on invalid votes. (If we did not, we would also
				// increase spam slots for backing validators for example - as validators have to
				// provide some opposing vote for dispute-distribution).
				free_spam_slots_available &=
					self.spam_slots.add_unconfirmed(session, candidate_hash, index);
			}
			// Only validity stating votes or validator had free spam slot?
			if !free_spam_slots_available {
				gum::debug!(
					target: LOG_TARGET,
					?candidate_hash,
					?session,
					?statements,
					"Rejecting import because of full spam slots."
				);
				return Ok(ImportStatementsResult::InvalidImport)
			}
		}

		if is_confirmed && !was_confirmed {
			// Former spammers have not been spammers after all:
			self.spam_slots.clear(&(session, candidate_hash));
		}

		// Check if newly disputed.
		let is_disputed = !votes.valid.is_empty() && !votes.invalid.is_empty();
		let concluded_valid = votes.valid.len() >= supermajority_threshold;
		let concluded_invalid = votes.invalid.len() >= supermajority_threshold;

		// Participate in dispute if the imported vote was not local, we did not vote before either
		// and we actually have keys to issue a local vote.
		if !is_local && !voted_already && is_disputed && !controlled_indices.is_empty() {
			let priority = ParticipationPriority::with_priority_if(is_included);
			gum::trace!(
				target: LOG_TARGET,
				candidate_hash = ?candidate_receipt.hash(),
				?priority,
				"Queuing participation for candidate"
			);
			if priority.is_priority() {
				self.metrics.on_queued_priority_participation();
			} else {
				self.metrics.on_queued_best_effort_participation();
			}
			// Participate whenever the imported vote was local & we did not had no cast
			// previously:
			let r = self
				.participation
				.queue_participation(
					ctx,
					priority,
					ParticipationRequest::new(candidate_receipt, session, n_validators),
				)
				.await;
			log_error(r)?;
		}

		let prev_status = recent_disputes.get(&(session, candidate_hash)).map(|x| x.clone());

		let status = if is_disputed {
			let status = recent_disputes.entry((session, candidate_hash)).or_insert_with(|| {
				gum::info!(
					target: LOG_TARGET,
					?candidate_hash,
					session,
					"New dispute initiated for candidate.",
				);
				DisputeStatus::active()
			});

			if is_confirmed {
				*status = status.confirm();
			}

			// Note: concluded-invalid overwrites concluded-valid,
			// so we do this check first. Dispute state machine is
			// non-commutative.
			if concluded_valid {
				*status = status.concluded_for(now);
			}

			if concluded_invalid {
				*status = status.concluded_against(now);
			}

			Some(*status)
		} else {
			None
		};

		if status != prev_status {
			if prev_status.is_none() {
				self.metrics.on_open();
			}

			if !was_concluded_valid && concluded_valid {
				gum::info!(
					target: LOG_TARGET,
					?candidate_hash,
					session,
					"Dispute on candidate concluded with 'valid' result",
				);
				self.metrics.on_concluded_valid();
			}

			if !was_concluded_invalid && concluded_invalid {
				gum::info!(
					target: LOG_TARGET,
					?candidate_hash,
					session,
					"Dispute on candidate concluded with 'invalid' result",
				);
				self.metrics.on_concluded_invalid();
			}

			// Only write when updated:
			overlay_db.write_recent_disputes(recent_disputes);
		}

		// Only write when votes have changed.
		if votes_changed {
			overlay_db.write_candidate_votes(session, candidate_hash, votes.into());
		}

		Ok(ImportStatementsResult::ValidImport)
	}

	async fn issue_local_statement<Context>(
		&mut self,
		ctx: &mut Context,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		candidate_hash: CandidateHash,
		candidate_receipt: CandidateReceipt,
		session: SessionIndex,
		valid: bool,
		now: Timestamp,
	) -> Result<()> {
		// Load session info.
		let info = match self.rolling_session_window.session_info(session) {
			None => {
				gum::warn!(
					target: LOG_TARGET,
					session,
					"Missing info for session which has an active dispute",
				);

				return Ok(())
			},
			Some(info) => info,
		};

		let validators = info.validators.clone();

		let votes = overlay_db
			.load_candidate_votes(session, &candidate_hash)?
			.map(CandidateVotes::from)
			.unwrap_or_else(|| CandidateVotes {
				candidate_receipt: candidate_receipt.clone(),
				valid: Vec::new(),
				invalid: Vec::new(),
			});

		// Sign a statement for each validator index we control which has
		// not already voted. This should generally be maximum 1 statement.
		let voted_indices = votes.voted_indices();
		let mut statements = Vec::new();

		let voted_indices: HashSet<_> = voted_indices.into_iter().collect();
		let controlled_indices = find_controlled_validator_indices(&self.keystore, &validators[..]);
		for index in controlled_indices {
			if voted_indices.contains(&index) {
				continue
			}

			let keystore = self.keystore.clone() as Arc<_>;
			let res = SignedDisputeStatement::sign_explicit(
				&keystore,
				valid,
				candidate_hash,
				session,
				validators[index.0 as usize].clone(),
			)
			.await;

			match res {
				Ok(Some(signed_dispute_statement)) => {
					statements.push((signed_dispute_statement, index));
				},
				Ok(None) => {},
				Err(e) => {
					gum::error!(
					target: LOG_TARGET,
					err = ?e,
					"Encountered keystore error while signing dispute statement",
					);
				},
			}
		}

		// Get our message out:
		for (statement, index) in &statements {
			let dispute_message =
				match make_dispute_message(info, &votes, statement.clone(), *index) {
					Err(err) => {
						gum::debug!(target: LOG_TARGET, ?err, "Creating dispute message failed.");
						continue
					},
					Ok(dispute_message) => dispute_message,
				};

			ctx.send_message(DisputeDistributionMessage::SendDispute(dispute_message)).await;
		}

		// Do import
		if !statements.is_empty() {
			match self
				.handle_import_statements(
					ctx,
					overlay_db,
					candidate_hash,
					MaybeCandidateReceipt::Provides(candidate_receipt),
					session,
					statements,
					now,
				)
				.await?
			{
				ImportStatementsResult::InvalidImport => {
					gum::error!(
						target: LOG_TARGET,
						?candidate_hash,
						?session,
						"`handle_import_statements` considers our own votes invalid!"
					);
				},
				ImportStatementsResult::ValidImport => {
					gum::trace!(
						target: LOG_TARGET,
						?candidate_hash,
						?session,
						"`handle_import_statements` successfully imported our vote!"
					);
				},
			}
		}

		Ok(())
	}
}

/// Messages to be handled in this subsystem.
enum MuxedMessage {
	/// Messages from other subsystems.
	Subsystem(FromOrchestra<DisputeCoordinatorMessage>),
	/// Messages from participation workers.
	Participation(participation::WorkerMessage),
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
impl MuxedMessage {
	async fn receive<Context>(
		ctx: &mut Context,
		from_sender: &mut participation::WorkerMessageReceiver,
	) -> FatalResult<Self> {
		// We are only fusing here to make `select` happy, in reality we will quit if the stream
		// ends.
		let from_overseer = ctx.recv().fuse();
		futures::pin_mut!(from_overseer, from_sender);
		futures::select!(
			msg = from_overseer => Ok(Self::Subsystem(msg.map_err(FatalError::SubsystemReceive)?)),
			msg = from_sender.next() => Ok(Self::Participation(msg.ok_or(FatalError::ParticipationWorkerReceiverExhausted)?)),
		)
	}
}

// Returns 'true' if no other vote by that validator was already
// present and 'false' otherwise. Same semantics as `HashSet`.
fn insert_into_statement_vec<T>(
	vec: &mut Vec<(T, ValidatorIndex, ValidatorSignature)>,
	tag: T,
	val_index: ValidatorIndex,
	val_signature: ValidatorSignature,
) -> bool {
	let pos = match vec.binary_search_by_key(&val_index, |x| x.1) {
		Ok(_) => return false, // no duplicates needed.
		Err(p) => p,
	};

	vec.insert(pos, (tag, val_index, val_signature));
	true
}

#[derive(Debug, Clone)]
enum MaybeCandidateReceipt {
	/// Directly provides the candiate receipt.
	Provides(CandidateReceipt),
	/// Assumes it was seen before by means of seconded message.
	AssumeBackingVotePresent,
}

#[derive(Debug, thiserror::Error)]
enum DisputeMessageCreationError {
	#[error("There was no opposite vote available")]
	NoOppositeVote,
	#[error("Found vote had an invalid validator index that could not be found")]
	InvalidValidatorIndex,
	#[error("Statement found in votes had invalid signature.")]
	InvalidStoredStatement,
	#[error(transparent)]
	InvalidStatementCombination(DisputeMessageCheckError),
}

fn make_dispute_message(
	info: &SessionInfo,
	votes: &CandidateVotes,
	our_vote: SignedDisputeStatement,
	our_index: ValidatorIndex,
) -> std::result::Result<DisputeMessage, DisputeMessageCreationError> {
	let validators = &info.validators;

	let (valid_statement, valid_index, invalid_statement, invalid_index) =
		if let DisputeStatement::Valid(_) = our_vote.statement() {
			let (statement_kind, validator_index, validator_signature) =
				votes.invalid.get(0).ok_or(DisputeMessageCreationError::NoOppositeVote)?.clone();
			let other_vote = SignedDisputeStatement::new_checked(
				DisputeStatement::Invalid(statement_kind),
				our_vote.candidate_hash().clone(),
				our_vote.session_index(),
				validators
					.get(validator_index.0 as usize)
					.ok_or(DisputeMessageCreationError::InvalidValidatorIndex)?
					.clone(),
				validator_signature,
			)
			.map_err(|()| DisputeMessageCreationError::InvalidStoredStatement)?;
			(our_vote, our_index, other_vote, validator_index)
		} else {
			let (statement_kind, validator_index, validator_signature) =
				votes.valid.get(0).ok_or(DisputeMessageCreationError::NoOppositeVote)?.clone();
			let other_vote = SignedDisputeStatement::new_checked(
				DisputeStatement::Valid(statement_kind),
				our_vote.candidate_hash().clone(),
				our_vote.session_index(),
				validators
					.get(validator_index.0 as usize)
					.ok_or(DisputeMessageCreationError::InvalidValidatorIndex)?
					.clone(),
				validator_signature,
			)
			.map_err(|()| DisputeMessageCreationError::InvalidStoredStatement)?;
			(other_vote, validator_index, our_vote, our_index)
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

/// Determine the the best block and its block number.
/// Assumes `block_descriptions` are sorted from the one
/// with the lowest `BlockNumber` to the highest.
fn determine_undisputed_chain(
	overlay_db: &mut OverlayedBackend<'_, impl Backend>,
	base_number: BlockNumber,
	base_hash: Hash,
	block_descriptions: Vec<BlockDescription>,
) -> Result<(BlockNumber, Hash)> {
	let last = block_descriptions
		.last()
		.map(|e| (base_number + block_descriptions.len() as BlockNumber, e.block_hash))
		.unwrap_or((base_number, base_hash));

	// Fast path for no disputes.
	let recent_disputes = match overlay_db.load_recent_disputes()? {
		None => return Ok(last),
		Some(a) if a.is_empty() => return Ok(last),
		Some(a) => a,
	};

	let is_possibly_invalid = |session, candidate_hash| {
		recent_disputes
			.get(&(session, candidate_hash))
			.map_or(false, |status| status.is_possibly_invalid())
	};

	for (i, BlockDescription { session, candidates, .. }) in block_descriptions.iter().enumerate() {
		if candidates.iter().any(|c| is_possibly_invalid(*session, *c)) {
			if i == 0 {
				return Ok((base_number, base_hash))
			} else {
				return Ok((base_number + i as BlockNumber, block_descriptions[i - 1].block_hash))
			}
		}
	}

	Ok(last)
}

fn find_controlled_validator_indices(
	keystore: &LocalKeystore,
	validators: &[ValidatorId],
) -> HashSet<ValidatorIndex> {
	let mut controlled = HashSet::new();
	for (index, validator) in validators.iter().enumerate() {
		if keystore.key_pair::<ValidatorPair>(validator).ok().flatten().is_none() {
			continue
		}

		controlled.insert(ValidatorIndex(index as _));
	}

	controlled
}
