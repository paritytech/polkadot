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

//! The provisioner is responsible for assembling a relay chain block
//! from a set of available parachain candidates of its choice.

#![deny(missing_docs, unused_crate_dependencies, unused_results)]

use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use sp_keystore::SyncCryptoStorePtr;
use polkadot_node_subsystem::{
	jaeger, PerLeafSpan, SubsystemSender,
	errors::ChainApiError,
	messages::{
		CandidateBackingMessage, CandidateSelectionMessage, CollatorProtocolMessage,
		RuntimeApiRequest,
	},
};
use polkadot_node_subsystem_util::{
	self as util, request_from_runtime, request_validator_groups, JobSubsystem,
	JobTrait, JobSender, Validator, metrics::{self, prometheus},
};
use polkadot_primitives::v1::{
	CandidateReceipt, CollatorId, CoreState, CoreIndex, Hash, Id as ParaId, BlockNumber,
};
use polkadot_node_primitives::{SignedFullStatement, PoV};
use std::{pin::Pin, sync::Arc};
use thiserror::Error;

const LOG_TARGET: &'static str = "parachain::candidate-selection";

/// A per-block job in the candidate selection subsystem.
pub struct CandidateSelectionJob {
	assignment: ParaId,
	receiver: mpsc::Receiver<CandidateSelectionMessage>,
	metrics: Metrics,
	seconded_candidate: Option<CollatorId>,
}

/// Errors in the candidate selection subsystem.
#[derive(Debug, Error)]
pub enum Error {
	/// An error in utilities.
	#[error(transparent)]
	Util(#[from] util::Error),
	/// An error receiving on a oneshot channel.
	#[error(transparent)]
	OneshotRecv(#[from] oneshot::Canceled),
	/// An error interacting with the chain API.
	#[error(transparent)]
	ChainApi(#[from] ChainApiError),
}

macro_rules! try_runtime_api {
	($x: expr) => {
		match $x {
			Ok(x) => x,
			Err(e) => {
				tracing::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to fetch runtime API data for job",
				);

				// We can't do candidate selection work if we don't have the
				// requisite runtime API data. But these errors should not take
				// down the node.
				return Ok(());
			}
		}
	}
}

impl JobTrait for CandidateSelectionJob {
	type ToJob = CandidateSelectionMessage;
	type Error = Error;
	type RunArgs = SyncCryptoStorePtr;
	type Metrics = Metrics;

	const NAME: &'static str = "CandidateSelectionJob";

	#[tracing::instrument(skip(keystore, metrics, receiver, sender), fields(subsystem = LOG_TARGET))]
	fn run<S: SubsystemSender>(
		relay_parent: Hash,
		span: Arc<jaeger::Span>,
		keystore: Self::RunArgs,
		metrics: Self::Metrics,
		receiver: mpsc::Receiver<CandidateSelectionMessage>,
		mut sender: JobSender<S>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		let span = PerLeafSpan::new(span, "candidate-selection");
		async move {
			let _span = span.child("query-runtime")
				.with_relay_parent(relay_parent)
				.with_stage(jaeger::Stage::CandidateSelection);
			let (groups, cores) = futures::try_join!(
				request_validator_groups(relay_parent, &mut sender).await,
				request_from_runtime(
					relay_parent,
					&mut sender,
					|tx| RuntimeApiRequest::AvailabilityCores(tx),
				).await,
			)?;

			let (validator_groups, group_rotation_info) = try_runtime_api!(groups);
			let cores = try_runtime_api!(cores);

			drop(_span);
			let _span = span.child("validator-construction")
				.with_relay_parent(relay_parent)
				.with_stage(jaeger::Stage::CandidateSelection);

			let n_cores = cores.len();

			let validator = match Validator::new(relay_parent, keystore.clone(), &mut sender).await {
				Ok(validator) => validator,
				Err(util::Error::NotAValidator) => return Ok(()),
				Err(err) => return Err(Error::Util(err)),
			};

			let assignment_span = span.child("find-assignment")
				.with_relay_parent(relay_parent)
				.with_stage(jaeger::Stage::CandidateSelection);

			#[derive(Debug)]
			enum AssignmentState {
				Unassigned,
				Scheduled(ParaId),
				Occupied(BlockNumber),
				Free,
			}

			let mut assignment = AssignmentState::Unassigned;

			for (idx, core) in cores.into_iter().enumerate() {
				let core_index = CoreIndex(idx as _);
				let group_index = group_rotation_info.group_for_core(core_index, n_cores);
				if let Some(g) = validator_groups.get(group_index.0 as usize) {
					if g.contains(&validator.index()) {
						match core {
							CoreState::Scheduled(scheduled) => {
								assignment = AssignmentState::Scheduled(scheduled.para_id);
							}
							CoreState::Occupied(occupied) => {
								// Ignore prospective assignments on occupied cores
								// for the time being.
								assignment = AssignmentState::Occupied(occupied.occupied_since);
							}
							CoreState::Free => {
								assignment = AssignmentState::Free;
							}
						}
						break;
					}
				}
			}

			let (assignment, assignment_span) = match assignment {
				AssignmentState::Scheduled(assignment) => {
					let assignment_span = assignment_span
					.with_string_tag("assigned", "true")
					.with_para_id(assignment);

					(assignment, assignment_span)
				}
				assignment => {
					let _assignment_span = assignment_span.with_string_tag("assigned", "false");

					let validator_index = validator.index();
					let validator_id = validator.id();

					tracing::debug!(
						target: LOG_TARGET,
						?relay_parent,
						?validator_index,
						?validator_id,
						?assignment,
						"No assignment. Will not select candidate."
					);

					return Ok(())
				}
			};

			drop(assignment_span);

			CandidateSelectionJob::new(assignment, metrics, receiver)
				.run_loop(&span, sender.subsystem_sender())
				.await
		}.boxed()
	}
}

impl CandidateSelectionJob {
	fn new(
		assignment: ParaId,
		metrics: Metrics,
		receiver: mpsc::Receiver<CandidateSelectionMessage>,
	) -> Self {
		Self {
			receiver,
			metrics,
			assignment,
			seconded_candidate: None,
		}
	}

	async fn run_loop(
		&mut self,
		span: &jaeger::Span,
		sender: &mut impl SubsystemSender,
	) -> Result<(), Error> {
		let span = span.child("run-loop")
			.with_stage(jaeger::Stage::CandidateSelection);

		loop {
			match self.receiver.next().await  {
				Some(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator_id,
				)) => {
					let _span = span.child("handle-collation");
					self.handle_collation(sender, relay_parent, para_id, collator_id).await;
				}
				Some(CandidateSelectionMessage::Invalid(
					_relay_parent,
					candidate_receipt,
				)) => {
					let _span = span.child("handle-invalid")
						.with_stage(jaeger::Stage::CandidateSelection)
						.with_candidate(candidate_receipt.hash())
						.with_relay_parent(_relay_parent);
					self.handle_invalid(sender, candidate_receipt).await;
				}
				Some(CandidateSelectionMessage::Seconded(_relay_parent, statement)) => {
					let _span = span.child("handle-seconded")
						.with_stage(jaeger::Stage::CandidateSelection)
						.with_candidate(statement.payload().candidate_hash())
						.with_relay_parent(_relay_parent);
					self.handle_seconded(sender, statement).await;
				}
				None => break,
			}
		}

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self, sender), fields(subsystem = LOG_TARGET))]
	async fn handle_collation(
		&mut self,
		sender: &mut impl SubsystemSender,
		relay_parent: Hash,
		para_id: ParaId,
		collator_id: CollatorId,
	) {
		let _timer = self.metrics.time_handle_collation();

		if self.assignment != para_id {
			tracing::info!(
				target: LOG_TARGET,
				"Collator {:?} sent a collation outside of our assignment {:?}",
				collator_id,
				para_id,
			);
			forward_invalidity_note(&collator_id, sender).await;
			return;
		}

		if self.seconded_candidate.is_none() {
			let (candidate_receipt, pov) =
				match get_collation(
					relay_parent,
					para_id,
					collator_id.clone(),
					sender,
				).await {
					Ok(response) => response,
					Err(err) => {
						tracing::warn!(
							target: LOG_TARGET,
							err = ?err,
							"failed to get collation from collator protocol subsystem",
						);
						return;
					}
				};

			second_candidate(
				relay_parent,
				candidate_receipt,
				pov,
				sender,
				&self.metrics,
			).await;
			self.seconded_candidate = Some(collator_id);
		}
	}

	#[tracing::instrument(level = "trace", skip(self, sender), fields(subsystem = LOG_TARGET))]
	async fn handle_invalid(
		&mut self,
		sender: &mut impl SubsystemSender,
		candidate_receipt: CandidateReceipt,
	) {
		let _timer = self.metrics.time_handle_invalid();

		let received_from = match &self.seconded_candidate {
			Some(peer) => peer,
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					"received invalidity notice for a candidate we don't remember seconding"
				);
				return;
			}
		};
		tracing::info!(
			target: LOG_TARGET,
			candidate_receipt = ?candidate_receipt,
			"received invalidity note for candidate",
		);

		forward_invalidity_note(received_from, sender).await;
		self.metrics.on_invalid_selection();
	}

	async fn handle_seconded(
		&mut self,
		sender: &mut impl SubsystemSender,
		statement: SignedFullStatement,
	) {
		let received_from = match &self.seconded_candidate {
			Some(peer) => peer,
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					"received seconded notice for a candidate we don't remember seconding"
				);
				return;
			}
		};
		tracing::debug!(
			target: LOG_TARGET,
			statement = ?statement,
			"received seconded note for candidate",
		);

		sender
			.send_message(CollatorProtocolMessage::NoteGoodCollation(received_from.clone()).into())
			.await;

		sender.send_message(
			CollatorProtocolMessage::NotifyCollationSeconded(received_from.clone(), statement).into()
		).await;
	}
}

// get a collation from the Collator Protocol subsystem
//
// note that this gets an owned clone of the sender; that's becuase unlike `forward_invalidity_note`, it's expected to take a while longer
#[tracing::instrument(level = "trace", skip(sender), fields(subsystem = LOG_TARGET))]
async fn get_collation(
	relay_parent: Hash,
	para_id: ParaId,
	collator_id: CollatorId,
	sender: &mut impl SubsystemSender,
) -> Result<(CandidateReceipt, PoV), Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(CollatorProtocolMessage::FetchCollation(
			relay_parent,
			collator_id,
			para_id,
			tx,
		).into())
		.await;

	rx.await.map_err(Into::into)
}

async fn second_candidate(
	relay_parent: Hash,
	candidate_receipt: CandidateReceipt,
	pov: PoV,
	sender: &mut impl SubsystemSender,
	metrics: &Metrics,
) {
	sender
		.send_message(CandidateBackingMessage::Second(
			relay_parent,
			candidate_receipt,
			pov,
		).into())
		.await;

	metrics.on_second();
}

async fn forward_invalidity_note(
	received_from: &CollatorId,
	sender: &mut impl SubsystemSender,
) {
	sender
		.send_message(CollatorProtocolMessage::ReportCollator(received_from.clone()).into())
		.await
}

#[derive(Clone)]
struct MetricsInner {
	seconds: prometheus::Counter<prometheus::U64>,
	invalid_selections: prometheus::Counter<prometheus::U64>,
	handle_collation: prometheus::Histogram,
	handle_invalid: prometheus::Histogram,
}

/// Candidate selection metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_second(&self) {
		if let Some(metrics) = &self.0 {
			metrics.seconds.inc();
		}
	}

	fn on_invalid_selection(&self) {
		if let Some(metrics) = &self.0 {
			metrics.invalid_selections.inc();
		}
	}

	/// Provide a timer for `handle_collation` which observes on drop.
	fn time_handle_collation(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.handle_collation.start_timer())
	}

	/// Provide a timer for `handle_invalid` which observes on drop.
	fn time_handle_invalid(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.handle_invalid.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			seconds: prometheus::register(
				prometheus::Counter::with_opts(
					prometheus::Opts::new(
						"candidate_selection_seconds_total",
						"Number of Candidate Selection subsystem seconding events.",
					),
				)?,
				registry,
			)?,
			invalid_selections: prometheus::register(
				prometheus::Counter::with_opts(
					prometheus::Opts::new(
						"candidate_selection_invalid_selections_total",
						"Number of Candidate Selection subsystem seconding selections which proved to be invalid.",
					),
				)?,
				registry,
			)?,
			handle_collation: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_candidate_selection_handle_collation",
						"Time spent within `candidate_selection::handle_collation`",
					)
				)?,
				registry,
			)?,
			handle_invalid: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_candidate_selection:handle_invalid",
						"Time spent within `candidate_selection::handle_invalid`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

/// The candidate selection subsystem.
pub type CandidateSelectionSubsystem<Spawner> = JobSubsystem<CandidateSelectionJob, Spawner>;

#[cfg(test)]
mod tests {
	use super::*;
	use futures::lock::Mutex;
	use polkadot_node_primitives::BlockData;
	use polkadot_node_subsystem::messages::AllMessages;
	use sp_core::crypto::Public;
	use std::sync::Arc;

	fn test_harness<Preconditions, TestBuilder, Test, Postconditions>(
		preconditions: Preconditions,
		test: TestBuilder,
		postconditions: Postconditions,
	) where
		Preconditions: FnOnce(&mut CandidateSelectionJob),
		TestBuilder: FnOnce(mpsc::Sender<CandidateSelectionMessage>, mpsc::UnboundedReceiver<AllMessages>) -> Test,
		Test: Future<Output = ()>,
		Postconditions: FnOnce(CandidateSelectionJob, Result<(), Error>),
	{
		let (to_job_tx, to_job_rx) = mpsc::channel(0);
		let (mut from_job_tx, from_job_rx) = polkadot_node_subsystem_test_helpers::sender_receiver();
		let mut job = CandidateSelectionJob {
			assignment: 123.into(),
			receiver: to_job_rx,
			metrics: Default::default(),
			seconded_candidate: None,
		};

		preconditions(&mut job);
		let span = jaeger::Span::Disabled;
		let (_, (job, job_result)) = futures::executor::block_on(future::join(
			test(to_job_tx, from_job_rx),
			async move {
				let res = job.run_loop(&span, &mut from_job_tx).await;
				drop(from_job_tx);
				(job, res)
			},
		));

		postconditions(job, job_result);
	}

	/// when nothing is seconded so far, the collation is fetched and seconded
	#[test]
	fn fetches_and_seconds_a_collation() {
		let relay_parent = Hash::random();
		let para_id: ParaId = 123.into();
		let collator_id = CollatorId::from_slice(&(0..32).collect::<Vec<u8>>());
		let collator_id_clone = collator_id.clone();

		let candidate_receipt = CandidateReceipt::default();
		let pov = PoV {
			block_data: BlockData((0..32).cycle().take(256).collect()),
		};

		let was_seconded = Arc::new(Mutex::new(false));
		let was_seconded_clone = was_seconded.clone();

		test_harness(
			|_job| {},
			|mut to_job, mut from_job| async move {
				to_job
					.send(CandidateSelectionMessage::Collation(
						relay_parent,
						para_id,
						collator_id_clone.clone(),
					))
					.await
					.unwrap();
				std::mem::drop(to_job);

				while let Some(msg) = from_job.next().await {
					match msg {
						AllMessages::CollatorProtocol(CollatorProtocolMessage::FetchCollation(
							got_relay_parent,
							collator_id,
							got_para_id,
							return_sender,
						)) => {
							assert_eq!(got_relay_parent, relay_parent);
							assert_eq!(got_para_id, para_id);
							assert_eq!(collator_id, collator_id_clone);

							return_sender
								.send((candidate_receipt.clone(), pov.clone()))
								.unwrap();
						}
						AllMessages::CandidateBacking(CandidateBackingMessage::Second(
							got_relay_parent,
							got_candidate_receipt,
							got_pov,
						)) => {
							assert_eq!(got_relay_parent, relay_parent);
							assert_eq!(got_candidate_receipt, candidate_receipt);
							assert_eq!(got_pov, pov);

							*was_seconded_clone.lock().await = true;
						}
						other => panic!("unexpected message from job: {:?}", other),
					}
				}
			},
			|job, job_result| {
				assert!(job_result.is_ok());
				assert_eq!(job.seconded_candidate.unwrap(), collator_id);
			},
		);

		assert!(Arc::try_unwrap(was_seconded).unwrap().into_inner());
	}

	/// when something has been seconded, further collation notifications are ignored
	#[test]
	fn ignores_collation_notifications_after_the_first() {
		let relay_parent = Hash::random();
		let para_id: ParaId = 123.into();
		let prev_collator_id = CollatorId::from_slice(&(0..32).rev().collect::<Vec<u8>>());
		let collator_id = CollatorId::from_slice(&(0..32).collect::<Vec<u8>>());
		let collator_id_clone = collator_id.clone();

		let was_seconded = Arc::new(Mutex::new(false));
		let was_seconded_clone = was_seconded.clone();

		test_harness(
			|job| job.seconded_candidate = Some(prev_collator_id.clone()),
			|mut to_job, mut from_job| async move {
				to_job
					.send(CandidateSelectionMessage::Collation(
						relay_parent,
						para_id,
						collator_id_clone,
					))
					.await
					.unwrap();
				std::mem::drop(to_job);

				while let Some(msg) = from_job.next().await {
					match msg {
						AllMessages::CandidateBacking(CandidateBackingMessage::Second(
							_got_relay_parent,
							_got_candidate_receipt,
							_got_pov,
						)) => {
							*was_seconded_clone.lock().await = true;
						}
						other => panic!("unexpected message from job: {:?}", other),
					}
				}
			},
			|job, job_result| {
				assert!(job_result.is_ok());
				assert_eq!(job.seconded_candidate.unwrap(), prev_collator_id);
			},
		);

		assert!(!Arc::try_unwrap(was_seconded).unwrap().into_inner());
	}

	/// reports of invalidity from candidate backing are propagated
	#[test]
	fn propagates_invalidity_reports() {
		let relay_parent = Hash::random();
		let collator_id = CollatorId::from_slice(&(0..32).collect::<Vec<u8>>());
		let collator_id_clone = collator_id.clone();

		let candidate_receipt = CandidateReceipt::default();

		let sent_report = Arc::new(Mutex::new(false));
		let sent_report_clone = sent_report.clone();

		test_harness(
			|job| job.seconded_candidate = Some(collator_id.clone()),
			|mut to_job, mut from_job| async move {
				to_job
					.send(CandidateSelectionMessage::Invalid(relay_parent, candidate_receipt))
					.await
					.unwrap();

				std::mem::drop(to_job);

				while let Some(msg) = from_job.next().await {
					match msg {
						AllMessages::CollatorProtocol(CollatorProtocolMessage::ReportCollator(
							got_collator_id,
						)) => {
							assert_eq!(got_collator_id, collator_id_clone);

							*sent_report_clone.lock().await = true;
						}
						other => panic!("unexpected message from job: {:?}", other),
					}
				}
			},
			|job, job_result| {
				assert!(job_result.is_ok());
				assert_eq!(job.seconded_candidate.unwrap(), collator_id);
			},
		);

		assert!(Arc::try_unwrap(sent_report).unwrap().into_inner());
	}
}
