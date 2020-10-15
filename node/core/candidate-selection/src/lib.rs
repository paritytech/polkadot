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

#![deny(missing_docs)]

use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use polkadot_node_primitives::ValidationResult;
use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::{
		AllMessages, CandidateBackingMessage, CandidateSelectionMessage,
		CandidateValidationMessage, CollatorProtocolMessage,
	},
};
use polkadot_node_subsystem_util::{
	self as util, delegated_subsystem, JobTrait, ToJobTrait,
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{
	CandidateDescriptor, CandidateReceipt, CollatorId, Hash, Id as ParaId, PoV,
};
use std::{convert::TryFrom, pin::Pin, sync::Arc};

const TARGET: &'static str = "candidate_selection";

struct CandidateSelectionJob {
	sender: mpsc::Sender<FromJob>,
	receiver: mpsc::Receiver<ToJob>,
	metrics: Metrics,
	seconded_candidate: Option<CollatorId>,
}

/// This enum defines the messages that the provisioner is prepared to receive.
#[derive(Debug)]
pub enum ToJob {
	/// The provisioner message is the main input to the provisioner.
	CandidateSelection(CandidateSelectionMessage),
	/// This message indicates that the provisioner should shut itself down.
	Stop,
}

impl ToJobTrait for ToJob {
	const STOP: Self = Self::Stop;

	fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::CandidateSelection(csm) => csm.relay_parent(),
			Self::Stop => None,
		}
	}
}

impl TryFrom<AllMessages> for ToJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::CandidateSelection(csm) => Ok(Self::CandidateSelection(csm)),
			_ => Err(()),
		}
	}
}

impl From<CandidateSelectionMessage> for ToJob {
	fn from(csm: CandidateSelectionMessage) -> Self {
		Self::CandidateSelection(csm)
	}
}

#[derive(Debug)]
enum FromJob {
	Validation(CandidateValidationMessage),
	Backing(CandidateBackingMessage),
	Collator(CollatorProtocolMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::Validation(msg) => AllMessages::CandidateValidation(msg),
			FromJob::Backing(msg) => AllMessages::CandidateBacking(msg),
			FromJob::Collator(msg) => AllMessages::CollatorProtocol(msg),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::CandidateValidation(msg) => Ok(FromJob::Validation(msg)),
			AllMessages::CandidateBacking(msg) => Ok(FromJob::Backing(msg)),
			AllMessages::CollatorProtocol(msg) => Ok(FromJob::Collator(msg)),
			_ => Err(()),
		}
	}
}

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Sending(mpsc::SendError),
	#[from]
	Util(util::Error),
	#[from]
	OneshotRecv(oneshot::Canceled),
	#[from]
	ChainApi(ChainApiError),
	#[from]
	Runtime(RuntimeApiError),
}

impl JobTrait for CandidateSelectionJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = ();
	type Metrics = Metrics;

	const NAME: &'static str = "CandidateSelectionJob";

	/// Run a job for the parent block indicated
	//
	// this function is in charge of creating and executing the job's main loop
	fn run(
		_relay_parent: Hash,
		_run_args: Self::RunArgs,
		metrics: Self::Metrics,
		receiver: mpsc::Receiver<ToJob>,
		sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = CandidateSelectionJob::new(metrics, sender, receiver);

			// it isn't necessary to break run_loop into its own function,
			// but it's convenient to separate the concerns in this way
			job.run_loop().await
		}
		.boxed()
	}
}

impl CandidateSelectionJob {
	pub fn new(
		metrics: Metrics,
		sender: mpsc::Sender<FromJob>,
		receiver: mpsc::Receiver<ToJob>,
	) -> Self {
		Self {
			sender,
			receiver,
			metrics,
			seconded_candidate: None,
		}
	}

	async fn run_loop(mut self) -> Result<(), Error> {
		self.run_loop_borrowed().await
	}

	/// this function exists for testing and should not generally be used; use `run_loop` instead.
	async fn run_loop_borrowed(&mut self) -> Result<(), Error> {
		while let Some(msg) = self.receiver.next().await {
			match msg {
				ToJob::CandidateSelection(CandidateSelectionMessage::Collation(
					relay_parent,
					para_id,
					collator_id,
				)) => {
					self.handle_collation(relay_parent, para_id, collator_id)
						.await;
				}
				ToJob::CandidateSelection(CandidateSelectionMessage::Invalid(
					_,
					candidate_receipt,
				)) => {
					self.handle_invalid(candidate_receipt).await;
				}
				ToJob::Stop => break,
			}
		}

		// closing the sender here means that we don't deadlock in tests
		self.sender.close_channel();

		Ok(())
	}

	async fn handle_collation(
		&mut self,
		relay_parent: Hash,
		para_id: ParaId,
		collator_id: CollatorId,
	) {
		if self.seconded_candidate.is_none() {
			let (candidate_receipt, pov) =
				match get_collation(
					relay_parent,
					para_id,
					collator_id.clone(),
					self.sender.clone(),
				).await {
					Ok(response) => response,
					Err(err) => {
						log::warn!(
							target: TARGET,
							"failed to get collation from collator protocol subsystem: {:?}",
							err
						);
						return;
					}
				};

			let pov = Arc::new(pov);

			if !candidate_is_valid(
				candidate_receipt.descriptor.clone(),
				pov.clone(),
				self.sender.clone(),
			)
			.await
			{
				return;
			}

			let pov = if let Ok(pov) = Arc::try_unwrap(pov) {
				pov
			} else {
				log::warn!(target: TARGET, "Arc unwrapping is expected to succeed, the other fns should have already run to completion by now.");
				return;
			};

			match second_candidate(
				relay_parent,
				candidate_receipt,
				pov,
				&mut self.sender,
				&self.metrics,
			)
			.await
			{
				Err(err) => log::warn!(target: TARGET, "failed to second a candidate: {:?}", err),
				Ok(()) => self.seconded_candidate = Some(collator_id),
			}
		}
	}

	async fn handle_invalid(&mut self, candidate_receipt: CandidateReceipt) {
		let received_from = match &self.seconded_candidate {
			Some(peer) => peer,
			None => {
				log::warn!(
					target: TARGET,
					"received invalidity notice for a candidate we don't remember seconding"
				);
				return;
			}
		};
		log::info!(
			target: TARGET,
			"received invalidity note for candidate {:?}",
			candidate_receipt
		);

		let result =
			if let Err(err) = forward_invalidity_note(received_from, &mut self.sender).await {
				log::warn!(
					target: TARGET,
					"failed to forward invalidity note: {:?}",
					err
				);
				Err(())
			} else {
				Ok(())
			};
		self.metrics.on_invalid_selection(result);
	}
}

// get a collation from the Collator Protocol subsystem
//
// note that this gets an owned clone of the sender; that's becuase unlike `forward_invalidity_note`, it's expected to take a while longer
async fn get_collation(
	relay_parent: Hash,
	para_id: ParaId,
	collator_id: CollatorId,
	mut sender: mpsc::Sender<FromJob>,
) -> Result<(CandidateReceipt, PoV), Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send(FromJob::Collator(CollatorProtocolMessage::FetchCollation(
			relay_parent,
			collator_id,
			para_id,
			tx,
		)))
		.await?;
	rx.await.map_err(Into::into)
}

// find out whether a candidate is valid or not
async fn candidate_is_valid(
	candidate_descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	sender: mpsc::Sender<FromJob>,
) -> bool {
	std::matches!(
		candidate_is_valid_inner(candidate_descriptor, pov, sender).await,
		Ok(true)
	)
}

// find out whether a candidate is valid or not, with a worse interface
// the external interface is worse, but the internal implementation is easier
async fn candidate_is_valid_inner(
	candidate_descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	mut sender: mpsc::Sender<FromJob>,
) -> Result<bool, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send(FromJob::Validation(
			CandidateValidationMessage::ValidateFromChainState(candidate_descriptor, pov, tx),
		))
		.await?;
	Ok(std::matches!(rx.await, Ok(Ok(ValidationResult::Valid(_)))))
}

async fn second_candidate(
	relay_parent: Hash,
	candidate_receipt: CandidateReceipt,
	pov: PoV,
	sender: &mut mpsc::Sender<FromJob>,
	metrics: &Metrics,
) -> Result<(), Error> {
	match sender
		.send(FromJob::Backing(CandidateBackingMessage::Second(
			relay_parent,
			candidate_receipt,
			pov,
		)))
		.await
	{
		Err(err) => {
			log::warn!(target: TARGET, "failed to send a seconding message");
			metrics.on_second(Err(()));
			Err(err.into())
		}
		Ok(_) => {
			metrics.on_second(Ok(()));
			Ok(())
		}
	}
}

async fn forward_invalidity_note(
	received_from: &CollatorId,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<(), Error> {
	sender
		.send(FromJob::Collator(CollatorProtocolMessage::ReportCollator(
			received_from.clone(),
		)))
		.await
		.map_err(Into::into)
}

#[derive(Clone)]
struct MetricsInner {
	seconds: prometheus::CounterVec<prometheus::U64>,
	invalid_selections: prometheus::CounterVec<prometheus::U64>,
}

/// Candidate selection metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_second(&self, result: Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			let label = if result.is_ok() { "succeeded" } else { "failed" };
			metrics.seconds.with_label_values(&[label]).inc();
		}
	}

	fn on_invalid_selection(&self, result: Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			let label = if result.is_ok() { "succeeded" } else { "failed" };
			metrics.invalid_selections.with_label_values(&[label]).inc();
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			seconds: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"candidate_selection_invalid_selections_total",
						"Number of Candidate Selection subsystem seconding selections which proved to be invalid.",
					),
					&["succeeded", "failed"],
				)?,
				registry,
			)?,
			invalid_selections: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"candidate_selection_invalid_selections_total",
						"Number of Candidate Selection subsystem seconding selections which proved to be invalid.",
					),
					&["succeeded", "failed"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

delegated_subsystem!(CandidateSelectionJob((), Metrics) <- ToJob as CandidateSelectionSubsystem);

#[cfg(test)]
mod tests {
	use super::*;
	use futures::lock::Mutex;
	use polkadot_node_primitives::ValidationOutputs;
	use polkadot_primitives::v1::{BlockData, HeadData, PersistedValidationData};
	use sp_core::crypto::Public;

	fn test_harness<Preconditions, TestBuilder, Test, Postconditions>(
		preconditions: Preconditions,
		test: TestBuilder,
		postconditions: Postconditions,
	) where
		Preconditions: FnOnce(&mut CandidateSelectionJob),
		TestBuilder: FnOnce(mpsc::Sender<ToJob>, mpsc::Receiver<FromJob>) -> Test,
		Test: Future<Output = ()>,
		Postconditions: FnOnce(CandidateSelectionJob, Result<(), Error>),
	{
		let (to_job_tx, to_job_rx) = mpsc::channel(0);
		let (from_job_tx, from_job_rx) = mpsc::channel(0);
		let mut job = CandidateSelectionJob {
			sender: from_job_tx,
			receiver: to_job_rx,
			metrics: Default::default(),
			seconded_candidate: None,
		};

		preconditions(&mut job);

		let (_, job_result) = futures::executor::block_on(future::join(
			test(to_job_tx, from_job_rx),
			job.run_loop_borrowed(),
		));

		postconditions(job, job_result);
	}

	fn default_validation_outputs() -> ValidationOutputs {
		let head_data: Vec<u8> = (0..32).rev().cycle().take(256).collect();
		let parent_head_data = head_data
			.iter()
			.copied()
			.map(|x| x.saturating_sub(1))
			.collect();

		ValidationOutputs {
			head_data: HeadData(head_data),
			validation_data: PersistedValidationData {
				parent_head: HeadData(parent_head_data),
				block_number: 123,
				hrmp_mqc_heads: Vec::new(),
			},
			upward_messages: Vec::new(),
			fees: 0,
			new_validation_code: None,
		}
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
					.send(ToJob::CandidateSelection(
						CandidateSelectionMessage::Collation(
							relay_parent,
							para_id,
							collator_id_clone.clone(),
						),
					))
					.await
					.unwrap();
				std::mem::drop(to_job);

				while let Some(msg) = from_job.next().await {
					match msg {
						FromJob::Collator(CollatorProtocolMessage::FetchCollation(
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
						FromJob::Validation(
							CandidateValidationMessage::ValidateFromChainState(
								got_candidate_descriptor,
								got_pov,
								return_sender,
							),
						) => {
							assert_eq!(got_candidate_descriptor, candidate_receipt.descriptor);
							assert_eq!(got_pov.as_ref(), &pov);

							return_sender
								.send(Ok(ValidationResult::Valid(default_validation_outputs())))
								.unwrap();
						}
						FromJob::Backing(CandidateBackingMessage::Second(
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
					.send(ToJob::CandidateSelection(
						CandidateSelectionMessage::Collation(
							relay_parent,
							para_id,
							collator_id_clone,
						),
					))
					.await
					.unwrap();
				std::mem::drop(to_job);

				while let Some(msg) = from_job.next().await {
					match msg {
						FromJob::Backing(CandidateBackingMessage::Second(
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
					.send(ToJob::CandidateSelection(
						CandidateSelectionMessage::Invalid(relay_parent, candidate_receipt),
					))
					.await
					.unwrap();
				std::mem::drop(to_job);

				while let Some(msg) = from_job.next().await {
					match msg {
						FromJob::Collator(CollatorProtocolMessage::ReportCollator(
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
