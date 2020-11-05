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
	messages::{
		AllMessages, AvailabilityStoreMessage, BitfieldDistributionMessage,
		BitfieldSigningMessage, CandidateBackingMessage, RuntimeApiMessage, RuntimeApiRequest,
	},
	errors::RuntimeApiError,
};
use polkadot_node_subsystem_util::{
	self as util, JobManager, JobTrait, ToJobTrait, Validator,
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{AvailabilityBitfield, CoreState, Hash, ValidatorIndex};
use std::{convert::TryFrom, pin::Pin, time::Duration, iter::FromIterator};
use wasm_timer::{Delay, Instant};
use thiserror::Error;

/// Delay between starting a bitfield signing job and its attempting to create a bitfield.
const JOB_DELAY: Duration = Duration::from_millis(1500);
const LOG_TARGET: &str = "bitfield_signing";

/// Each `BitfieldSigningJob` prepares a signed bitfield for a single relay parent.
pub struct BitfieldSigningJob;

/// Messages which a `BitfieldSigningJob` is prepared to receive.
#[allow(missing_docs)]
pub enum ToJob {
	BitfieldSigning(BitfieldSigningMessage),
	Stop,
}

impl ToJobTrait for ToJob {
	const STOP: Self = ToJob::Stop;

	fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::BitfieldSigning(bsm) => bsm.relay_parent(),
			Self::Stop => None,
		}
	}
}

impl TryFrom<AllMessages> for ToJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::BitfieldSigning(bsm) => Ok(ToJob::BitfieldSigning(bsm)),
			_ => Err(()),
		}
	}
}

impl From<BitfieldSigningMessage> for ToJob {
	fn from(bsm: BitfieldSigningMessage) -> ToJob {
		ToJob::BitfieldSigning(bsm)
	}
}

/// Messages which may be sent from a `BitfieldSigningJob`.
#[allow(missing_docs)]
#[derive(Debug, derive_more::From)]
pub enum FromJob {
	AvailabilityStore(AvailabilityStoreMessage),
	BitfieldDistribution(BitfieldDistributionMessage),
	CandidateBacking(CandidateBackingMessage),
	RuntimeApi(RuntimeApiMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::AvailabilityStore(asm) => AllMessages::AvailabilityStore(asm),
			FromJob::BitfieldDistribution(bdm) => AllMessages::BitfieldDistribution(bdm),
			FromJob::CandidateBacking(cbm) => AllMessages::CandidateBacking(cbm),
			FromJob::RuntimeApi(ram) => AllMessages::RuntimeApi(ram),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::AvailabilityStore(asm) => Ok(Self::AvailabilityStore(asm)),
			AllMessages::BitfieldDistribution(bdm) => Ok(Self::BitfieldDistribution(bdm)),
			AllMessages::CandidateBacking(cbm) => Ok(Self::CandidateBacking(cbm)),
			AllMessages::RuntimeApi(ram) => Ok(Self::RuntimeApi(ram)),
			_ => Err(()),
		}
	}
}

/// Errors we may encounter in the course of executing the `BitfieldSigningSubsystem`.
#[derive(Debug, Error)]
pub enum Error {
	/// error propagated from the utility subsystem
	#[error(transparent)]
	Util(#[from] util::Error),
	/// io error
	#[error(transparent)]
	Io(#[from] std::io::Error),
	/// a one shot channel was canceled
	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),
	/// a mspc channel failed to send
	#[error(transparent)]
	MpscSend(#[from] mpsc::SendError),
	/// the runtime API failed to return what we wanted
	#[error(transparent)]
	Runtime(#[from] RuntimeApiError),
	/// the keystore failed to process signing request
	#[error("Keystore failed: {0:?}")]
	Keystore(KeystoreError),
}

/// If there is a candidate pending availability, query the Availability Store
/// for whether we have the availability chunk for our validator index.
async fn get_core_availability(
	relay_parent: Hash,
	core: CoreState,
	validator_idx: ValidatorIndex,
	sender: &Mutex<&mut mpsc::Sender<FromJob>>,
) -> Result<bool, Error> {
	if let CoreState::Occupied(core) = core {
		let (tx, rx) = oneshot::channel();
		sender
			.lock()
			.await
			.send(
				RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::CandidatePendingAvailability(core.para_id, tx),
				).into(),
			)
			.await?;

		let committed_candidate_receipt = match rx.await? {
			Ok(Some(ccr)) => ccr,
			Ok(None) => return Ok(false),
			Err(e) => {
				// Don't take down the node on runtime API errors.
				log::warn!(target: LOG_TARGET, "Encountered a runtime API error: {:?}", e);
				return Ok(false);
			}
		};
		let (tx, rx) = oneshot::channel();
		sender
			.lock()
			.await
			.send(
				AvailabilityStoreMessage::QueryChunkAvailability(
					committed_candidate_receipt.hash(),
					validator_idx,
					tx,
				).into(),
			)
			.await?;
		return rx.await.map_err(Into::into);
	}

	Ok(false)
}

/// delegates to the v1 runtime API
async fn get_availability_cores(relay_parent: Hash, sender: &mut mpsc::Sender<FromJob>) -> Result<Vec<CoreState>, Error> {
	let (tx, rx) = oneshot::channel();
	sender.send(RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::AvailabilityCores(tx)).into()).await?;
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
async fn construct_availability_bitfield(
	relay_parent: Hash,
	validator_idx: ValidatorIndex,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<AvailabilityBitfield, Error> {
	// get the set of availability cores from the runtime
	let availability_cores = get_availability_cores(relay_parent, sender).await?;

	// Wrap the sender in a Mutex to share it between the futures.
	//
	// We use a `Mutex` here to not `clone` the sender inside the future, because
	// cloning the sender will always increase the capacity of the channel by one.
	// (for the lifetime of the sender)
	let sender = Mutex::new(sender);

	// Handle all cores concurrently
	// `try_join_all` returns all results in the same order as the input futures.
	let results = future::try_join_all(
		availability_cores.into_iter().map(|core| get_core_availability(relay_parent, core, validator_idx, &sender)),
	).await?;

	Ok(AvailabilityBitfield(FromIterator::from_iter(results)))
}

#[derive(Clone)]
struct MetricsInner {
	bitfields_signed_total: prometheus::Counter<prometheus::U64>,
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
		};
		Ok(Metrics(Some(metrics)))
	}
}

impl JobTrait for BitfieldSigningJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = SyncCryptoStorePtr;
	type Metrics = Metrics;

	const NAME: &'static str = "BitfieldSigningJob";

	/// Run a job for the parent block indicated
	fn run(
		relay_parent: Hash,
		keystore: Self::RunArgs,
		metrics: Self::Metrics,
		_receiver: mpsc::Receiver<ToJob>,
		mut sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
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

			let bitfield =
				match construct_availability_bitfield(relay_parent, validator.index(), &mut sender).await
			{
				Err(Error::Runtime(runtime_err)) => {
					// Don't take down the node on runtime API errors.
					log::warn!(target: LOG_TARGET, "Encountered a runtime API error: {:?}", runtime_err);
					return Ok(());
				}
				Err(err) => return Err(err),
				Ok(bitfield) => bitfield,
			};

			let signed_bitfield = validator
				.sign(keystore.clone(), bitfield)
				.await
				.map_err(|e| Error::Keystore(e))?;
			metrics.on_bitfield_signed();

			sender
				.send(BitfieldDistributionMessage::DistributeBitfield(relay_parent, signed_bitfield).into())
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
	use polkadot_primitives::v1::{OccupiedCore};
	use FromJob::*;

	fn occupied_core(para_id: u32) -> CoreState {
		CoreState::Occupied(OccupiedCore {
			para_id: para_id.into(),
			group_responsible: para_id.into(),
			next_up_on_available: None,
			occupied_since: 100_u32,
			time_out_at: 200_u32,
			next_up_on_time_out: None,
			availability: Default::default(),
		})
	}

	#[test]
	fn construct_availability_bitfield_works() {
		block_on(async move {
			let (mut sender, mut receiver) = mpsc::channel(10);
			let relay_parent = Hash::default();
			let validator_index = 1u32;

			let future = construct_availability_bitfield(relay_parent, validator_index, &mut sender).fuse();
			pin_mut!(future);

			loop {
				futures::select! {
					m = receiver.next() => match m.unwrap() {
						RuntimeApi(RuntimeApiMessage::Request(rp, RuntimeApiRequest::AvailabilityCores(tx))) => {
							assert_eq!(relay_parent, rp);
							tx.send(Ok(vec![CoreState::Free, occupied_core(1), occupied_core(2)])).unwrap();
						},
						RuntimeApi(
							RuntimeApiMessage::Request(rp, RuntimeApiRequest::CandidatePendingAvailability(para_id, tx))
						) => {
							assert_eq!(relay_parent, rp);

							if para_id == 1.into() {
								tx.send(Ok(Some(Default::default()))).unwrap();
							} else {
								tx.send(Ok(None)).unwrap();
							}
						},
						AvailabilityStore(AvailabilityStoreMessage::QueryChunkAvailability(_, vidx, tx)) => {
							assert_eq!(validator_index, vidx);

							tx.send(true).unwrap();
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
