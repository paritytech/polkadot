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
	metrics::{self, prometheus},
};
use polkadot_node_subsystem_util::{self as util, delegated_subsystem, JobTrait, ToJobTrait};
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

		Ok(())
	}

	async fn handle_collation(
		&mut self,
		relay_parent: Hash,
		para_id: ParaId,
		collator_id: CollatorId,
	) {
		if self.seconded_candidate.is_none() {
			let (candidate_receipt, pov) = match get_collation(
				relay_parent,
				para_id,
				self.sender.clone(),
			)
			.await
			{
				Ok(response) => response,
				Err(err) => {
					log::warn!(target: TARGET, "failed to get collation from collator protocol subsystem: {:?}", err);
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
				Err(err) => {
					log::warn!(target: TARGET, "failed to second a candidate: {:?}", err)
				}
				Ok(()) => self.seconded_candidate = Some(collator_id),
			}
		}
	}

	async fn handle_invalid(&mut self, candidate_receipt: CandidateReceipt) {
		let received_from = match &self.seconded_candidate {
			Some(peer) => peer,
			None => {
				log::warn!(target: TARGET, "received invalidity notice for a candidate we don't remember seconding");
				return;
			}
		};
		log::info!(target: TARGET, "received invalidity note for candidate {:?}", candidate_receipt);

		let succeeded = if let Err(err) = forward_invalidity_note(received_from, &mut self.sender).await {
			log::warn!(target: TARGET, "failed to forward invalidity note: {:?}", err);
			false
		} else {
			true
		};
		self.metrics.on_invalid_selection(succeeded);
	}
}

// get a collation from the Collator Protocol subsystem
//
// note that this gets an owned clone of the sender; that's becuase unlike `forward_invalidity_note`, it's expected to take a while longer
async fn get_collation(
	relay_parent: Hash,
	para_id: ParaId,
	mut sender: mpsc::Sender<FromJob>,
) -> Result<(CandidateReceipt, PoV), Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send(FromJob::Collator(CollatorProtocolMessage::FetchCollation(
			relay_parent,
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
			metrics.on_second(false);
			Err(err.into())
		}
		Ok(_) => {
			metrics.on_second(true);
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

/// Candidate backing metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_second(&self, succeeded: bool) {
		if let Some(metrics) = &self.0 {
			let label = if succeeded { "succeeded" } else { "failed" };
			metrics.seconds.with_label_values(&[label]).inc();
		}
	}

	fn on_invalid_selection(&self, succeeded: bool) {
		if let Some(metrics) = &self.0 {
			let label = if succeeded { "succeeded" } else { "failed" };
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
