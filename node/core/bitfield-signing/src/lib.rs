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

//! The bitfield signing subsystem produces `SignedAvailabilityBitfield`s once per block.

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]
#![recursion_limit="256"]

use futures::{channel::{mpsc, oneshot}, lock::Mutex, prelude::*, future, Future};
use sp_keystore::{Error as KeystoreError, SyncCryptoStorePtr};
use polkadot_node_subsystem::{
	jaeger, PerLeafSpan,
	messages::{
		AllMessages, AvailabilityStoreMessage, BitfieldDistributionMessage,
		BitfieldSigningMessage, RuntimeApiMessage, RuntimeApiRequest,
	},
	errors::RuntimeApiError,
};
use polkadot_node_subsystem_util::{
	self as util, JobManager, JobTrait, Validator, FromJobCommand, metrics::{self, prometheus},
};
use polkadot_primitives::v1::{AvailabilityBitfield, CoreState, Hash, ValidatorIndex};
use std::{pin::Pin, time::Duration, iter::FromIterator, sync::Arc};
use wasm_timer::{Delay, Instant};

/// Delay between starting a bitfield signing job and its attempting to create a bitfield.
const JOB_DELAY: Duration = Duration::from_millis(1500);
const LOG_TARGET: &str = "bitfield_signing";

/// Each `BitfieldSigningJob` prepares a signed bitfield for a single relay parent.
pub struct BitfieldSigningJob;

/// Errors we may encounter in the course of executing the `BitfieldSigningSubsystem`.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	Util(#[from] util::Error),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	MpscSend(#[from] mpsc::SendError),

	#[error(transparent)]
	Runtime(#[from] RuntimeApiError),

	#[error("Keystore failed: {0:?}")]
	Keystore(KeystoreError),
}

/// If there is a candidate pending availability, query the Availability Store
/// for whether we have the availability chunk for our validator index.
#[tracing::instrument(level = "trace", skip(sender, span), fields(subsystem = LOG_TARGET))]
async fn get_core_availability(
	core: &CoreState,
	validator_idx: ValidatorIndex,
	sender: &Mutex<&mut mpsc::Sender<FromJobCommand>>,
	span: &jaeger::Span,
) -> Result<bool, Error> {
	if let &CoreState::Occupied(ref core) = core {
		let _span = span.child("query-chunk-availability");

		let (tx, rx) = oneshot::channel();
		sender
			.lock()
			.await
			.send(
				AllMessages::from(AvailabilityStoreMessage::QueryChunkAvailability(
					core.candidate_hash,
					validator_idx,
					tx,
				)).into(),
			)
			.await?;

		let res = rx.await.map_err(Into::into);

		tracing::trace!(
			target: LOG_TARGET,
			para_id = %core.para_id(),
			availability = ?res,
			?core.candidate_hash,
			"Candidate availability",
		);

		res
	} else {
		Ok(false)
	}
}

/// delegates to the v1 runtime API
async fn get_availability_cores(
	relay_parent: Hash,
	sender: &mut mpsc::Sender<FromJobCommand>,
) -> Result<Vec<CoreState>, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send(AllMessages::from(RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::AvailabilityCores(tx))).into())
		.await?;
	match rx.await {
		Ok(Ok(out)) => Ok(out),
		Ok(Err(runtime_err)) => Err(runtime_err.into()),
		Err(err) => Err(err.into())
	}
}

/// - get the list of core states from the runtime
/// - for each core, concurrently determine chunk availability (see `get_core_availability`)
/// - return the bitfield if there were no errors at any point in this process
///   (otherwise, it's prone to false negatives)
#[tracing::instrument(level = "trace", skip(sender, span), fields(subsystem = LOG_TARGET))]
async fn construct_availability_bitfield(
	relay_parent: Hash,
	span: &jaeger::Span,
	validator_idx: ValidatorIndex,
	sender: &mut mpsc::Sender<FromJobCommand>,
) -> Result<AvailabilityBitfield, Error> {
	// get the set of availability cores from the runtime
	let availability_cores = {
		let _span = span.child("get-availability-cores");
		get_availability_cores(relay_parent, sender).await?
	};

	// Wrap the sender in a Mutex to share it between the futures.
	//
	// We use a `Mutex` here to not `clone` the sender inside the future, because
	// cloning the sender will always increase the capacity of the channel by one.
	// (for the lifetime of the sender)
	let sender = Mutex::new(sender);

	// Handle all cores concurrently
	// `try_join_all` returns all results in the same order as the input futures.
	let results = future::try_join_all(
		availability_cores.iter()
			.map(|core| get_core_availability(core, validator_idx, &sender, span)),
	).await?;

	tracing::debug!(
		target: LOG_TARGET,
		"Signing Bitfield for {} cores: {:?}",
		availability_cores.len(),
		results,
	);

	Ok(AvailabilityBitfield(FromIterator::from_iter(results)))
}

#[derive(Clone)]
struct MetricsInner {
	bitfields_signed_total: prometheus::Counter<prometheus::U64>,
	run: prometheus::Histogram,
}

/// Bitfield signing metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_bitfield_signed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.bitfields_signed_total.inc();
		}
	}

	/// Provide a timer for `prune_povs` which observes on drop.
	fn time_run(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.run.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			bitfields_signed_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_bitfields_signed_total",
					"Number of bitfields signed.",
				)?,
				registry,
			)?,
			run: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_bitfield_signing_run",
						"Time spent within `bitfield_signing::run`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

impl JobTrait for BitfieldSigningJob {
	type ToJob = BitfieldSigningMessage;
	type Error = Error;
	type RunArgs = SyncCryptoStorePtr;
	type Metrics = Metrics;

	const NAME: &'static str = "BitfieldSigningJob";

	/// Run a job for the parent block indicated
	#[tracing::instrument(skip(span, keystore, metrics, _receiver, sender), fields(subsystem = LOG_TARGET))]
	fn run(
		relay_parent: Hash,
		span: Arc<jaeger::Span>,
		keystore: Self::RunArgs,
		metrics: Self::Metrics,
		_receiver: mpsc::Receiver<BitfieldSigningMessage>,
		mut sender: mpsc::Sender<FromJobCommand>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		let metrics = metrics.clone();
		async move {
			let span = PerLeafSpan::new(span, "bitfield-signing");
			let _span = span.child("delay");
			let wait_until = Instant::now() + JOB_DELAY;

			// now do all the work we can before we need to wait for the availability store
			// if we're not a validator, we can just succeed effortlessly
			let validator = match Validator::new(relay_parent, keystore.clone(), sender.clone()).await {
				Ok(validator) => validator,
				Err(util::Error::NotAValidator) => return Ok(()),
				Err(err) => return Err(Error::Util(err)),
			};

			// wait a bit before doing anything else
			Delay::new_at(wait_until).await?;

			// this timer does not appear at the head of the function because we don't want to include
			// JOB_DELAY each time.
			let _timer = metrics.time_run();

			drop(_span);
			let span_availability = span.child("availability");

			let bitfield =
				match construct_availability_bitfield(
					relay_parent,
					&span_availability,
					validator.index(),
					&mut sender,
				).await
			{
				Err(Error::Runtime(runtime_err)) => {
					// Don't take down the node on runtime API errors.
					tracing::warn!(target: LOG_TARGET, err = ?runtime_err, "Encountered a runtime API error");
					return Ok(());
				}
				Err(err) => return Err(err),
				Ok(bitfield) => bitfield,
			};

			drop(span_availability);
			let _span = span.child("signing");

			let signed_bitfield = match validator.sign(keystore.clone(), bitfield)
				.await
				.map_err(|e| Error::Keystore(e))?
			{
				Some(b) => b,
				None => {
					tracing::error!(
						target: LOG_TARGET,
						"Key was found at construction, but while signing it could not be found.",
					);
					return Ok(());
				}
			};

			metrics.on_bitfield_signed();

			drop(_span);
			let _span = span.child("gossip");

			sender
				.send(
					AllMessages::from(
						BitfieldDistributionMessage::DistributeBitfield(relay_parent, signed_bitfield),
					).into(),
				)
				.await
				.map_err(Into::into)
		}
		.boxed()
	}
}

/// BitfieldSigningSubsystem manages a number of bitfield signing jobs.
pub type BitfieldSigningSubsystem<Spawner, Context> = JobManager<Spawner, Context, BitfieldSigningJob>;

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{pin_mut, executor::block_on};
	use polkadot_primitives::v1::{CandidateHash, OccupiedCore};

	fn occupied_core(para_id: u32, candidate_hash: CandidateHash) -> CoreState {
		CoreState::Occupied(OccupiedCore {
			group_responsible: para_id.into(),
			next_up_on_available: None,
			occupied_since: 100_u32,
			time_out_at: 200_u32,
			next_up_on_time_out: None,
			availability: Default::default(),
			candidate_hash,
			candidate_descriptor: Default::default(),
		})
	}

	#[test]
	fn construct_availability_bitfield_works() {
		block_on(async move {
			let (mut sender, mut receiver) = mpsc::channel(10);
			let relay_parent = Hash::default();
			let validator_index = ValidatorIndex(1u32);

			let future = construct_availability_bitfield(
				relay_parent,
				&jaeger::Span::Disabled,
				validator_index,
				&mut sender,
			).fuse();
			pin_mut!(future);

			let hash_a = CandidateHash(Hash::repeat_byte(1));
			let hash_b = CandidateHash(Hash::repeat_byte(2));

			loop {
				futures::select! {
					m = receiver.next() => match m.unwrap() {
						FromJobCommand::SendMessage(
							AllMessages::RuntimeApi(
								RuntimeApiMessage::Request(rp, RuntimeApiRequest::AvailabilityCores(tx)),
							),
						) => {
							assert_eq!(relay_parent, rp);
							tx.send(Ok(vec![CoreState::Free, occupied_core(1, hash_a), occupied_core(2, hash_b)])).unwrap();
						},
						FromJobCommand::SendMessage(
							AllMessages::AvailabilityStore(
								AvailabilityStoreMessage::QueryChunkAvailability(c_hash, vidx, tx),
							),
						) => {
							assert_eq!(validator_index, vidx);

							tx.send(c_hash == hash_a).unwrap();
						},
						o => panic!("Unknown message: {:?}", o),
					},
					r = future => match r {
						Ok(r) => {
							assert!(!r.0.get(0).unwrap());
							assert!(r.0.get(1).unwrap());
							assert!(!r.0.get(2).unwrap());
							break
						},
						Err(e) => panic!("Failed: {:?}", e),
					},
				}
			}
		});
	}
}
