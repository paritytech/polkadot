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

//! The provisioner is responsible for assembling a set of items, from which the
//! runtime will pick a subset and create a relay chain block.

#![deny(missing_docs, unused_crate_dependencies)]

use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use futures_timer::Delay;
use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	jaeger,
	messages::{
		CandidateBackingMessage, DisputeCoordinatorMessage, ProvisionableData,
		ProvisionerInherentData, ProvisionerMessage,
	},
	PerLeafSpan, SubsystemSender,
};
use polkadot_node_subsystem_util::{self as util, JobSender, JobSubsystem, JobTrait};
use polkadot_primitives::v1::{
	BackedCandidate, CandidateHash, CandidateReceipt, DisputeStatement, DisputeStatementSet, Hash,
	Id as ParaId, MultiDisputeStatementSet, SignedAvailabilityBitfield,
	SignedAvailabilityBitfields,
};
use std::{collections::HashSet, pin::Pin, sync::Arc};
use thiserror::Error;

mod metrics;

pub use self::metrics::*;

#[cfg(test)]
mod tests;

/// How long to wait before proposing.
const PRE_PROPOSE_TIMEOUT: std::time::Duration = core::time::Duration::from_millis(2000);

const LOG_TARGET: &str = "parachain::provisioner";

enum InherentAfter {
	Ready,
	Wait(Delay),
}

impl InherentAfter {
	fn new_from_now() -> Self {
		InherentAfter::Wait(Delay::new(PRE_PROPOSE_TIMEOUT))
	}

	fn is_ready(&self) -> bool {
		match *self {
			InherentAfter::Ready => true,
			InherentAfter::Wait(_) => false,
		}
	}

	async fn ready(&mut self) {
		match *self {
			InherentAfter::Ready => {
				// Make sure we never end the returned future.
				// This is required because the `select!` that calls this future will end in a busy loop.
				futures::pending!()
			},
			InherentAfter::Wait(ref mut d) => {
				d.await;
				*self = InherentAfter::Ready;
			},
		}
	}
}

/// A per-relay-parent job for the provisioning subsystem.
pub struct ProvisioningJob {
	relay_parent: Hash,
	receiver: mpsc::Receiver<ProvisionerMessage>,
	backed_candidates: Vec<CandidateReceipt>,
	signed_bitfields: Vec<SignedAvailabilityBitfield>,
	metrics: Metrics,
	inherent_after: InherentAfter,
	awaiting_inherent: Vec<oneshot::Sender<ProvisionerInherentData>>,
}

/// Errors in the provisioner.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	Util(#[from] util::Error),

	#[error("failed to get backed candidates")]
	CanceledBackedCandidates(#[source] oneshot::Canceled),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Runtime(#[from] RuntimeApiError),

	#[error("failed to send return message with Inherents")]
	InherentDataReturnChannel,
}

impl JobTrait for ProvisioningJob {
	type ToJob = ProvisionerMessage;
	type Error = Error;
	type RunArgs = ();
	type Metrics = Metrics;

	const NAME: &'static str = "provisioner-job";

	/// Run a job for the parent block indicated
	//
	// this function is in charge of creating and executing the job's main loop
	fn run<S: SubsystemSender>(
		relay_parent: Hash,
		span: Arc<jaeger::Span>,
		_run_args: Self::RunArgs,
		metrics: Self::Metrics,
		receiver: mpsc::Receiver<ProvisionerMessage>,
		mut sender: JobSender<S>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = ProvisioningJob::new(relay_parent, metrics, receiver);

			job.run_loop(sender.subsystem_sender(), PerLeafSpan::new(span, "provisioner"))
				.await
		}
		.boxed()
	}
}

impl ProvisioningJob {
	fn new(
		relay_parent: Hash,
		metrics: Metrics,
		receiver: mpsc::Receiver<ProvisionerMessage>,
	) -> Self {
		Self {
			relay_parent,
			receiver,
			backed_candidates: Vec::new(),
			signed_bitfields: Vec::new(),
			metrics,
			inherent_after: InherentAfter::new_from_now(),
			awaiting_inherent: Vec::new(),
		}
	}

	async fn run_loop(
		mut self,
		sender: &mut impl SubsystemSender,
		span: PerLeafSpan,
	) -> Result<(), Error> {
		loop {
			futures::select! {
				msg = self.receiver.next() => match msg {
					Some(ProvisionerMessage::RequestInherentData(_, return_sender)) => {
						let _span = span.child("req-inherent-data");
						let _timer = self.metrics.time_request_inherent_data();

						if self.inherent_after.is_ready() {
							self.send_inherent_data(sender, vec![return_sender]).await;
						} else {
							self.awaiting_inherent.push(return_sender);
						}
					}
					Some(ProvisionerMessage::ProvisionableData(_, data)) => {
						let span = span.child("provisionable-data");
						let _timer = self.metrics.time_provisionable_data();

						self.note_provisionable_data(&span, data);
					}
					None => break,
				},
				_ = self.inherent_after.ready().fuse() => {
					let _span = span.child("send-inherent-data");
					let return_senders = std::mem::take(&mut self.awaiting_inherent);
					if !return_senders.is_empty() {
						self.send_inherent_data(sender, return_senders).await;
					}
				}
			}
		}

		Ok(())
	}

	async fn send_inherent_data(
		&mut self,
		sender: &mut impl SubsystemSender,
		return_senders: Vec<oneshot::Sender<ProvisionerInherentData>>,
	) {
		if let Err(err) = send_inherent_data(
			self.relay_parent,
			self.signed_bitfields.clone(),
			self.backed_candidates.clone(),
			return_senders,
			sender,
		)
		.await
		{
			tracing::warn!(target: LOG_TARGET, err = ?err, "failed to assemble or send inherent data");
			self.metrics.on_inherent_data_request(Err(()));
		} else {
			self.metrics.on_inherent_data_request(Ok(()));
		}
	}

	fn note_provisionable_data(
		&mut self,
		span: &jaeger::Span,
		provisionable_data: ProvisionableData,
	) {
		match provisionable_data {
			ProvisionableData::Bitfield(_, signed_bitfield) =>
				self.signed_bitfields.push(signed_bitfield),
			ProvisionableData::BackedCandidate(backed_candidate) => {
				let _span = span
					.child("provisionable-backed")
					.with_para_id(backed_candidate.descriptor().para_id);
				self.backed_candidates.push(backed_candidate)
			},
			_ => {},
		}
	}
}

/// The provisioner is the subsystem best suited on the node side,
/// yet it lacks sufficient information to do weight based inherents limiting.
/// This does the minimalistic checks and forwards a most likely
/// too large set of bitfields, candidates, and dispute votes to
/// the runtime. The `fn create_inherent` in the runtime is responsible
/// to use a subset of these.
async fn send_inherent_data(
	relay_parent: Hash,
	bitfields: SignedAvailabilityBitfields,
	candidate_receipts: Vec<CandidateReceipt>,
	return_senders: Vec<oneshot::Sender<ProvisionerInherentData>>,
	from_job: &mut impl SubsystemSender,
) -> Result<(), Error> {
	let backed_candidates =
		collect_backed_candidates(candidate_receipts, relay_parent, from_job).await?;

	let disputes = collect_disputes(from_job).await?;

	let inherent_data = ProvisionerInherentData { bitfields, backed_candidates, disputes };

	for return_sender in return_senders {
		return_sender
			.send(inherent_data.clone())
			.map_err(|_data| Error::InherentDataReturnChannel)?;
	}

	Ok(())
}

/// Collect backed candidates with a matching `relay_parent`.
async fn collect_backed_candidates(
	candidate_receipts: Vec<CandidateReceipt>,
	relay_parent: Hash,
	sender: &mut impl SubsystemSender,
) -> Result<Vec<BackedCandidate>, Error> {
	let max_one_candidate_per_para = HashSet::<ParaId>::with_capacity(candidate_receipts.len());
	let selected_candidates = candidate_receipts
		.into_iter()
		.filter(|candidate_receipt| {
			// assure the follow up query `GetBackedCandidate` succeeds
			candidate_receipt.descriptor().relay_parent == relay_parent
		})
		.scan(max_one_candidate_per_para, |unique, candidate_receipt| {
			let para_id = candidate_receipt.descriptor().para_id;
			if unique.insert(para_id) {
				Some(candidate_receipt.hash())
			} else {
				tracing::debug!(
					target: LOG_TARGET,
					?para_id,
					"Duplicate candidate detected for para, only submitting one",
				);
				None
			}
		})
		.collect::<Vec<CandidateHash>>();

	// now get the backed candidates corresponding to these candidate receipts
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(
			CandidateBackingMessage::GetBackedCandidates(
				relay_parent,
				selected_candidates.clone(),
				tx,
			)
			.into(),
		)
		.await;
	let backed_candidates = rx.await.map_err(|err| Error::CanceledBackedCandidates(err))?;

	tracing::debug!(
		target: LOG_TARGET,
		"Selected {} backed candidates ready to be sanitized by the runtime",
		backed_candidates.len(),
	);

	Ok(backed_candidates)
}

async fn collect_disputes(
	sender: &mut impl SubsystemSender,
) -> Result<MultiDisputeStatementSet, Error> {
	let (tx, rx) = oneshot::channel();

	// We use `RecentDisputes` instead of `ActiveDisputes` because redundancy is fine.
	// It's heavier than `ActiveDisputes` but ensures that everything from the dispute
	// window gets on-chain, unlike `ActiveDisputes`.
	//
	// This should have no meaningful impact on performance on production networks for
	// two reasons:
	// 1. In large validator sets, a node might be a block author 1% or less of the time.
	//    this code-path only triggers in the case of being a block author.
	// 2. Disputes are expected to be rare because they come with heavy slashing.
	sender.send_message(DisputeCoordinatorMessage::RecentDisputes(tx).into()).await;

	// TODO: scrape concluded disputes from on-chain to limit the number of disputes
	// TODO: <https://github.com/paritytech/polkadot/issues/4329>
	let recent_disputes = match rx.await {
		Ok(r) => r,
		Err(oneshot::Canceled) => {
			tracing::debug!(
				target: LOG_TARGET,
				"Unable to gather recent disputes - subsystem disconnected?",
			);

			Vec::new()
		},
	};

	// Load all votes for all disputes from the coordinator.
	let dispute_candidate_votes = {
		let (tx, rx) = oneshot::channel();
		sender
			.send_message(
				DisputeCoordinatorMessage::QueryCandidateVotes(recent_disputes, tx).into(),
			)
			.await;

		match rx.await {
			Ok(v) => v,
			Err(oneshot::Canceled) => {
				tracing::debug!(
					target: LOG_TARGET,
					"Unable to query candidate votes - subsystem disconnected?",
				);
				Vec::new()
			},
		}
	};

	// Transform all `CandidateVotes` into `MultiDisputeStatementSet`.
	Ok(dispute_candidate_votes
		.into_iter()
		.map(|(session_index, candidate_hash, votes)| {
			let valid_statements =
				votes.valid.into_iter().map(|(s, i, sig)| (DisputeStatement::Valid(s), i, sig));

			let invalid_statements = votes
				.invalid
				.into_iter()
				.map(|(s, i, sig)| (DisputeStatement::Invalid(s), i, sig));

			DisputeStatementSet {
				candidate_hash,
				session: session_index,
				statements: valid_statements.chain(invalid_statements).collect(),
			}
		})
		.collect())
}

/// The provisioning subsystem.
pub type ProvisionerSubsystem<Spawner> = JobSubsystem<ProvisioningJob, Spawner>;
