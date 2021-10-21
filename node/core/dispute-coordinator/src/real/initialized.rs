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
	collections::HashSet,
	sync::Arc,
	time::{SystemTime, UNIX_EPOCH},
};

use futures::{
	channel::{mpsc, oneshot},
	FutureExt, TryFutureExt,
};
use kvdb::KeyValueDB;
use parity_scale_codec::{Decode, Encode, Error as CodecError};
use polkadot_node_primitives::{
	CandidateVotes, DisputeMessage, DisputeMessageCheckError, SignedDisputeStatement,
	DISPUTE_WINDOW,
};
use polkadot_node_subsystem::{
	messages::{
		BlockDescription, DisputeCoordinatorMessage, DisputeDistributionMessage,
		DisputeParticipationMessage, ImportStatementsResult, RuntimeApiMessage, RuntimeApiRequest,
	},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem,
	SubsystemContext, SubsystemError,
};
use polkadot_node_subsystem_util::rolling_session_window::{
	self, RollingSessionWindow, SessionWindowUpdate,
};
use polkadot_primitives::v1::{
	BlockNumber, CandidateHash, CandidateReceipt, CompactStatement, DisputeStatement,
	DisputeStatementSet, Hash, ScrapedOnChainVotes, SessionIndex, SessionInfo,
	ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorPair, ValidatorSignature,
};
use sc_keystore::LocalKeystore;

use crate::{Config, DisputeCoordinatorSubsystem, metrics::Metrics};
use backend::{Backend, OverlayedBackend};
use db::v1::{DbBackend, RecentDisputes};
use error::{FatalResult, Result};

use self::{
	error::{log_error, NonFatal},
	spam_slots::SpamSlots,
};

use super::{LOG_TARGET, ordering::{OrderingProvider, CandidateComparator}, participation::{self, Participation, WorkerMessageReceiver, WorkerMessageSender, ParticipationRequest}, spam_slots::SpamSlots, status::Clock, error::Fatal};

/// After the first active leaves update we transition to `Initialized` state.
///
/// Before the first active leaves update we can't really do much. We cannot check incoming
/// statements for validity, we cannot query orderings, we have no valid `RollingSessionWindow`,
/// ...
pub struct Initialized {
	config: Config,
	store: Arc<dyn KeyValueDB>,
	keystore: Arc<LocalKeystore>,

	rolling_session_window: RollingSessionWindow,
	highest_session: SessionIndex,
	spam_slots: SpamSlots,
	participation: Participation,
	ordering_provider: OrderingProvider,
	participation_receiver: WorkerMessageReceiver,
	metrics: Metrics,
}

impl Initialized {
	fn new(
		subsystem: DisputeCoordinatorSubsystem,
		leaf: ActivatedLeaf,
		rolling_session_window: RollingSessionWindow,
		spam_slots: SpamSlots,
		ordering_provider: OrderingProvider,
	) -> Self {
		let DisputeCoordinatorSubsystem { config, store, keystore, metrics } = subsystem;

		let (participation_sender, participation_receiver) = mpsc::channel(1);
		let participation = Participation::new(participation_sender);

		Self {
			config,
			store,
			keystore,
			rolling_session_window,
			highest_session: rolling_session_window.latest_session(),
			spam_slots,
			ordering_provider,
			participation,
			participation_receiver,
			metrics,
		}
	}

	async fn run<B, Context>(
		mut self,
		mut ctx: Context,
		mut backend: B,
        mut participations: Vec<(Option<CandidateComparator>, ParticipationRequest)>,
		clock: Box<dyn Clock>,
	) -> FatalResult<()>
	where
		Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
		Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
		B: Backend,
	{
		loop {
			let res = self.run_until_error(&mut ctx, &mut backend, &mut participations, &*clock).await;
			if let Ok(()) = res {
				tracing::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
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
        participations: &mut Vec<(Option<CandidateComparator>, ParticipationRequest)>,
		clock: &dyn Clock,
	) -> Result<()>
	where
		Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
		Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
		B: Backend,
	{
        for (comparator, request) in std::mem::take(participations).into_iter() {
            self.participation.queue_participation(ctx, comparator, request).await?;
        }

		loop {
			let mut overlay_db = OverlayedBackend::new(backend);
			match MuxedMessage::receive(ctx, &mut self.participation_receiver).await? {
			}
			{
				FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
					self.handle_new_activations(
						ctx,
						&mut overlay_db,
						update.activated.into_iter().map(|a| a.hash),
						clock.now(),
					)
					.await?;
				},
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_, _)) => {},
				FromOverseer::Communication { msg } =>
					self.handle_incoming(ctx, &mut overlay_db, msg, clock.now()).await?,
			}

			if !overlay_db.is_empty() {
				let ops = overlay_db.into_write_ops();
				backend.write(ops)?;
			}
		}
	}

	async fn handle_new_activations(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = DisputeCoordinatorMessage>
		          + overseer::SubsystemContext<Message = DisputeCoordinatorMessage>),
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		new_activations: impl IntoIterator<Item = Hash>,
		now: u64,
	) -> Result<()> {
		for new_leaf in new_activations {
			match self.rolling_session_window.cache_session_info_for_head(ctx, new_leaf).await {
				Err(e) => {
					tracing::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to update session cache for disputes",
					);
					continue
				},
				Ok(SessionWindowUpdate::Initialized { window_end, .. }) |
				Ok(SessionWindowUpdate::Advanced { new_window_end: window_end, .. }) => {
					let session = window_end;
					if self.highest_session.map_or(true, |s| s < session) {
						tracing::trace!(
							target: LOG_TARGET,
							session,
							"Observed new session. Pruning"
						);

						self.highest_session = Some(session);

						db::v1::note_current_session(overlay_db, session)?;
					}
				},
				Ok(SessionWindowUpdate::Unchanged) => {},
			};
			self.scrape_on_chain_votes(ctx, overlay_db, new_leaf, now).await?;
		}

		Ok(())
	}

	/// Scrapes on-chain votes (backing votes and concluded disputes) for a active leaf of the relay chain.
	async fn scrape_on_chain_votes(
		&mut self,
		ctx: &mut (impl SubsystemContext<Message = DisputeCoordinatorMessage>
		          + overseer::SubsystemContext<Message = DisputeCoordinatorMessage>),
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		new_leaf: Hash,
		now: u64,
	) -> Result<()> {
		// obtain the concluded disputes as well as the candidate backing votes
		// from the new leaf
		let ScrapedOnChainVotes { session, backing_validators_per_candidate, disputes } = {
			let (tx, rx) = oneshot::channel();
			ctx.send_message(RuntimeApiMessage::Request(
				new_leaf,
				RuntimeApiRequest::FetchOnChainVotes(tx),
			))
			.await;
			match rx.await {
				Ok(Ok(Some(val))) => val,
				Ok(Ok(None)) => {
					tracing::trace!(
						target: LOG_TARGET,
						relay_parent = ?new_leaf,
						"No on chain votes stored for relay chain leaf");
					return Ok(())
				},
				Ok(Err(e)) => {
					tracing::debug!(
						target: LOG_TARGET,
						relay_parent = ?new_leaf,
						error = ?e,
						"Could not retrieve on chain votes due to an API error");
					return Ok(())
				},
				Err(e) => {
					tracing::debug!(
						target: LOG_TARGET,
						relay_parent = ?new_leaf,
						error = ?e,
						"Could not retrieve onchain votes due to oneshot cancellation");
					return Ok(())
				},
			}
		};

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
				tracing::warn!(
					target: LOG_TARGET,
					relay_parent = ?new_leaf,
					"Could not retrieve session info from rolling session window");
				return Ok(())
			};

		// Scraped on-chain backing votes for the candidates with
		// the new active leaf as if we received them via gossip.
		for (candidate_receipt, backers) in backing_validators_per_candidate {
			let candidate_hash = candidate_receipt.hash();
			let statements = backers.into_iter().filter_map(|(validator_index, attestation)| {
				let validator_public: ValidatorId = session_info
					.validators
					.get(validator_index.0 as usize)
					.or_else(|| {
						tracing::error!(
							target: LOG_TARGET,
							relay_parent = ?new_leaf,
							"Missing public key for validator {:?}",
							&validator_index);
						None
					})
					.cloned()?;
				let validator_signature = attestation.signature().clone();
				let valid_statement_kind = match attestation.to_compact_statement(candidate_hash) {
					CompactStatement::Seconded(_) =>
						ValidDisputeStatementKind::BackingSeconded(new_leaf),
					CompactStatement::Valid(_) => ValidDisputeStatementKind::BackingValid(new_leaf),
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
			});
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
				ImportStatementsResult::ValidImport => tracing::trace!(target: LOG_TARGET,
																	   relay_parent = ?new_leaf,
																	   ?session,
																	   "Imported backing vote from on-chain"),
				ImportStatementsResult::InvalidImport => tracing::warn!(target: LOG_TARGET,
																		relay_parent = ?new_leaf,
																		?session,
																		"Attempted import of on-chain backing votes failed"),
			}
		}

		if disputes.is_empty() {
			return Ok(())
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
						tracing::warn!(
								target: LOG_TARGET,
								relay_parent = ?new_leaf,
								?session,
								"Could not retrieve session info from rolling session window for recently concluded dispute");
						return None
					};

					let validator_public: ValidatorId = session_info
						.validators
						.get(validator_index.0 as usize)
						.or_else(|| {
							tracing::error!(
								target: LOG_TARGET,
								relay_parent = ?new_leaf,
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
				ImportStatementsResult::ValidImport => tracing::trace!(target: LOG_TARGET,
																	   relay_parent = ?new_leaf,
																	   ?candidate_hash,
																	   ?session,
																	   "Imported statement of concluded dispute from on-chain"),
				ImportStatementsResult::InvalidImport => tracing::warn!(target: LOG_TARGET,
																		relay_parent = ?new_leaf,
																		?candidate_hash,
																		?session,
																		"Attempted import of on-chain statement of concluded dispute failed"),
			}
		}
		Ok(())
	}

	async fn handle_incoming(
		&mut self,
		ctx: &mut impl SubsystemContext,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		message: DisputeCoordinatorMessage,
		now: Timestamp,
	) -> Result<()> {
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
				pending_confirmation
					.send(outcome)
					.map_err(|_| NonFatal::DisputeImportOneshotSend)?;
			},
			DisputeCoordinatorMessage::RecentDisputes(rx) => {
				let recent_disputes = overlay_db.load_recent_disputes()?.unwrap_or_default();
				let _ = rx.send(recent_disputes.keys().cloned().collect());
			},
			DisputeCoordinatorMessage::ActiveDisputes(rx) => {
				let recent_disputes = overlay_db.load_recent_disputes()?.unwrap_or_default();
				let _ = rx.send(collect_active(recent_disputes, now));
			},
			DisputeCoordinatorMessage::QueryCandidateVotes(query, rx) => {
				let mut query_output = Vec::new();
				for (session_index, candidate_hash) in query.into_iter() {
					if let Some(v) =
						overlay_db.load_candidate_votes(session_index, &candidate_hash)?
					{
						query_output.push((session_index, candidate_hash, v.into()));
					} else {
						tracing::debug!(
							target: LOG_TARGET,
							session_index,
							"No votes found for candidate",
						);
					}
				}
				let _ = rx.send(query_output);
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
				let undisputed_chain = determine_undisputed_chain(
					overlay_db,
					base_number,
					base_hash,
					block_descriptions,
				)?;

				let _ = tx.send(undisputed_chain);
			},
		}

		Ok(())
	}
	async fn handle_import_statements(
		&mut self,
		ctx: &mut impl SubsystemContext,
		overlay_db: &mut OverlayedBackend<'_, impl Backend>,
		candidate_hash: CandidateHash,
		candidate_receipt: MaybeCandidateReceipt,
		session: SessionIndex,
		statements: impl IntoIterator<Item = (SignedDisputeStatement, ValidatorIndex)>,
		now: Timestamp,
	) -> Result<ImportStatementsResult> {
		if self.highest_session.map_or(true, |h| session + DISPUTE_WINDOW < h) {
			// It is not valid to participate in an ancient dispute (spam?).
			return Ok(ImportStatementsResult::InvalidImport)
		}

		let session_info = match self.rolling_session_window.session_info(session) {
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					session,
					"Missing info for session which has an active dispute",
				);

				return Ok(ImportStatementsResult::InvalidImport)
			},
			Some(info) => info,
		};
		let validators = session_info.validators.clone();

		let n_validators = validators.len();

		let supermajority_threshold =
			polkadot_primitives::v1::supermajority_threshold(n_validators);

		// In case we are not provided with a candidate receipt
		// we operate under the assumption, that a previous vote
		// which included a `CandidateReceipt` was seen.
		// This holds since every block is preceeded by the `Backing`-phase.
		//
		// There is one exception: A sufficiently sophisticated attacker could prevent
		// us from seeing the backing votes by witholding arbitrary blocks, and hence we do
		// not have a `CandidateReceipt` available.
		let mut votes = match overlay_db
			.load_candidate_votes(session, &candidate_hash)?
			.map(CandidateVotes::from)
		{
			Some(votes) => votes,
			None =>
				if let MaybeCandidateReceipt::Provides(candidate_receipt) = candidate_receipt {
					CandidateVotes { candidate_receipt, valid: Vec::new(), invalid: Vec::new() }
				} else {
					tracing::warn!(
						target: LOG_TARGET,
						session,
						"Missing info for session which has an active dispute",
					);
					return Ok(ImportStatementsResult::InvalidImport)
				},
		};
		let candidate_receipt = votes.candidate_receipt.clone();

		// Update candidate votes.
		for (statement, val_index) in statements {
			if validators
				.get(val_index.0 as usize)
				.map_or(true, |v| v != statement.validator_public())
			{
				tracing::debug!(
				target: LOG_TARGET,
				?val_index,
				session,
				claimed_key = ?statement.validator_public(),
				"Validator index doesn't match claimed key",
				);

				continue
			}

			match statement.statement().clone() {
				DisputeStatement::Valid(valid_kind) => {
					self.metrics.on_valid_vote();
					insert_into_statement_vec(
						&mut votes.valid,
						valid_kind,
						val_index,
						statement.validator_signature().clone(),
					);
				},
				DisputeStatement::Invalid(invalid_kind) => {
					self.metrics.on_invalid_vote();
					insert_into_statement_vec(
						&mut votes.invalid,
						invalid_kind,
						val_index,
						statement.validator_signature().clone(),
					);
				},
			}
		}

		// Check if newly disputed.
		let is_disputed = !votes.valid.is_empty() && !votes.invalid.is_empty();
		let concluded_valid = votes.valid.len() >= supermajority_threshold;
		let concluded_invalid = votes.invalid.len() >= supermajority_threshold;

		let mut recent_disputes = overlay_db.load_recent_disputes()?.unwrap_or_default();

		let prev_status = recent_disputes.get(&(session, candidate_hash)).map(|x| x.clone());

		let status = if is_disputed {
			let status = recent_disputes.entry((session, candidate_hash)).or_insert_with(|| {
				tracing::info!(
					target: LOG_TARGET,
					?candidate_hash,
					session,
					"New dispute initiated for candidate.",
				);
				DisputeStatus::active()
			});

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
			// This branch is only hit when the candidate is freshly disputed -
			// status was previously `None`, and now is not.
			if prev_status.is_none() && {
				let controlled_indices =
					find_controlled_validator_indices(&self.keystore, &validators);
				let voted_indices = votes.voted_indices();

				!controlled_indices.iter().all(|val_index| voted_indices.contains(&val_index))
			} {
				ctx.send_message(DisputeParticipationMessage::Participate {
					candidate_hash,
					candidate_receipt,
					session,
					n_validators: n_validators as u32,
				})
				.await;

				self.metrics.on_open();

				if concluded_valid {
					self.metrics.on_concluded_valid();
				}
				if concluded_invalid {
					self.metrics.on_concluded_invalid();
				}
			}

			// Only write when updated and vote is available.
			overlay_db.write_recent_disputes(recent_disputes);
		}

		overlay_db.write_candidate_votes(session, candidate_hash, votes.into());

		Ok(ImportStatementsResult::ValidImport)
	}

	async fn issue_local_statement(
		&mut self,
		ctx: &mut impl SubsystemContext,
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
				tracing::warn!(
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
					tracing::error!(
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
						tracing::debug!(
							target: LOG_TARGET,
							?err,
							"Creating dispute message failed."
						);
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
				.await
			{
				Err(_) => {
					tracing::error!(
							target: LOG_TARGET,
							?candidate_hash,
							?session,
							"pending confirmation receiver got dropped by `handle_import_statements` for our own votes!"
							);
				},
				Ok(ImportStatementsResult::InvalidImport) => {
					tracing::error!(
						target: LOG_TARGET,
						?candidate_hash,
						?session,
						"`handle_import_statements` considers our own votes invalid!"
					);
				},
				Ok(ImportStatementsResult::ValidImport) => {
					tracing::trace!(
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
#[derive(Debug)]
enum MuxedMessage {
	/// Messages from other subsystems.
	Subsystem(FromOverseer<DisputeDistributionMessage>),
	/// Messages from participation workers.
	Participation(participation::WorkerMessage),
}

impl MuxedMessage {
	async fn receive(
		ctx: &mut (impl SubsystemContext<Message = DisputeDistributionMessage>
		          + overseer::SubsystemContext<Message = DisputeDistributionMessage>),
		from_sender: &mut participation::WorkerMessageReceiver,
	) -> FatalResult<Self> {
		// We are only fusing here to make `select` happy, in reality we will quit if the stream
		// ends.
		let from_overseer = ctx.recv().fuse();
		futures::pin_mut!(from_overseer, from_sender);
		futures::select!(
			msg = from_overseer => MuxedMessage::Subsystem(msg.map_err(Fatal::SubsystemReceive)?),
			msg = from_sender.next() => MuxedMessage::Sender(msg.ok_or(Fatal::ParticipationWorkerReceiverExhausted)?),
		)
	}
}

fn insert_into_statement_vec<T>(
	vec: &mut Vec<(T, ValidatorIndex, ValidatorSignature)>,
	tag: T,
	val_index: ValidatorIndex,
	val_signature: ValidatorSignature,
) {
	let pos = match vec.binary_search_by_key(&val_index, |x| x.1) {
		Ok(_) => return, // no duplicates needed.
		Err(p) => p,
	};

	vec.insert(pos, (tag, val_index, val_signature));
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
