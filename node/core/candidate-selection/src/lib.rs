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
use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::{
		AllMessages, CandidateBackingMessage, CollatorProtocolMessage,
		CandidateSelectionMessage, CandidateValidationMessage, NetworkBridgeMessage, RuntimeApiMessage,
	},
	metrics::{self, prometheus},
};
use polkadot_node_subsystem_util::{
	self as util,
	delegated_subsystem,
	JobTrait, ToJobTrait,
};
use polkadot_primitives::v1::{
	CollatorId, Hash,
};
use std::{convert::TryFrom, pin::Pin};

struct CandidateSelectionJob {
	sender: mpsc::Sender<FromJob>,
	receiver: mpsc::Receiver<ToJob>,
	metrics: Metrics,
	seconded_candidate: Option<(Hash, CollatorId)>,
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
	Network(NetworkBridgeMessage),
	Collator(CollatorProtocolMessage),
	Runtime(RuntimeApiMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::Validation(msg) => AllMessages::CandidateValidation(msg),
			FromJob::Backing(msg) => AllMessages::CandidateBacking(msg),
			FromJob::Collator(msg) => AllMessages::CollatorProtocol(msg),
			FromJob::Network(msg) => AllMessages::NetworkBridge(msg),
			FromJob::Runtime(msg) => AllMessages::RuntimeApi(msg),
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
			AllMessages::NetworkBridge(msg) => Ok(FromJob::Network(msg)),
			AllMessages::RuntimeApi(msg) => Ok(FromJob::Runtime(msg)),
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
				ToJob::CandidateSelection(CandidateSelectionMessage::Invalid(_, receipt)) => {
					let (seconded_candidate, received_from) = match &self.seconded_candidate {
						Some((backed, peer)) => (backed, peer),
						None => {
							log::warn!(target: "candidate_selection", "received invalidity notice for a candidate ({}) we don't remember recommending for seconding", receipt.hash());
							continue
						},
					};
					log::info!(target: "candidate_selection", "received invalidity note for candidate {}", seconded_candidate);
					if let Err(err) = forward_invalidity_note(
						received_from.clone(),
						&mut self.sender,
					)
					.await
					{
						log::warn!(target: "candidate_selection", "failed to forward invalidity note: {:?}", err);
						self.metrics.on_invalid_selection(false);
					} else {
						self.metrics.on_invalid_selection(true);
					}
				}
				ToJob::Stop => break,
			}
		}

		Ok(())
	}
}

// The provisioner is the subsystem best suited to choosing which specific
// backed candidates and availability bitfields should be assembled into the
// block. To engage this functionality, a
// `CandidateSelectionMessage::RequestInherentData` is sent; the response is a set of
// non-conflicting candidates and the appropriate bitfields. Non-conflicting
// means that there are never two distinct parachain candidates included for
// the same parachain and that new parachain candidates cannot be included
// until the previous one either gets declared available or expired.
//
// The main complication here is going to be around handling
// occupied-core-assumptions. We might have candidates that are only
// includable when some bitfields are included. And we might have candidates
// that are not includable when certain bitfields are included.
//
// When we're choosing bitfields to include, the rule should be simple:
// maximize availability. So basically, include all bitfields. And then
// choose a coherent set of candidates along with that.
async fn forward_invalidity_note(
	received_from: CollatorId,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<(), Error> {
	sender.send(FromJob::Collator(CollatorProtocolMessage::ReportCollator(received_from))).await.map_err(Into::into)
}


#[derive(Clone)]
struct MetricsInner {
	invalid_selections: prometheus::CounterVec<prometheus::U64>,
}

/// Candidate backing metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
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
