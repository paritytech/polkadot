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

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::{
		AllMessages, ChainApiMessage, ProvisionableData, ProvisionerInherentData,
		ProvisionerMessage, RuntimeApiMessage,
	},
};
use polkadot_node_subsystem_util::{
	self as util,
	delegated_subsystem,
	request_availability_cores, request_persisted_validation_data, JobTrait, ToJobTrait,
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{
	BackedCandidate, BlockNumber, CoreState, Hash, OccupiedCoreAssumption,
	SignedAvailabilityBitfield,
};
use std::{collections::HashSet, convert::TryFrom, pin::Pin};
use thiserror::Error;

struct ProvisioningJob {
	relay_parent: Hash,
	sender: mpsc::Sender<FromJob>,
	receiver: mpsc::Receiver<ToJob>,
	provisionable_data_channels: Vec<mpsc::Sender<ProvisionableData>>,
	backed_candidates: Vec<BackedCandidate>,
	signed_bitfields: Vec<SignedAvailabilityBitfield>,
	metrics: Metrics,
}

/// This enum defines the messages that the provisioner is prepared to receive.
pub enum ToJob {
	/// The provisioner message is the main input to the provisioner.
	Provisioner(ProvisionerMessage),
	/// This message indicates that the provisioner should shut itself down.
	Stop,
}

impl ToJobTrait for ToJob {
	const STOP: Self = Self::Stop;

	fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::Provisioner(pm) => pm.relay_parent(),
			Self::Stop => None,
		}
	}
}

impl TryFrom<AllMessages> for ToJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::Provisioner(pm) => Ok(Self::Provisioner(pm)),
			_ => Err(()),
		}
	}
}

impl From<ProvisionerMessage> for ToJob {
	fn from(pm: ProvisionerMessage) -> Self {
		Self::Provisioner(pm)
	}
}

enum FromJob {
	ChainApi(ChainApiMessage),
	Runtime(RuntimeApiMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::ChainApi(cam) => AllMessages::ChainApi(cam),
			FromJob::Runtime(ram) => AllMessages::RuntimeApi(ram),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::ChainApi(chain) => Ok(FromJob::ChainApi(chain)),
			AllMessages::RuntimeApi(runtime) => Ok(FromJob::Runtime(runtime)),
			_ => Err(()),
		}
	}
}

#[derive(Debug, Error)]
enum Error {
	#[error(transparent)]
	Util(#[from] util::Error),

	#[error(transparent)]
	OneshotRecv(#[from] oneshot::Canceled),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Runtime(#[from] RuntimeApiError),

	#[error("Failed to send message to ChainAPI")]
	ChainApiMessageSend(#[source] mpsc::SendError),

	#[error("Failed to send return message with Inherents")]
	InherentDataReturnChannel,
}

impl JobTrait for ProvisioningJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = ();
	type Metrics = Metrics;

	const NAME: &'static str = "ProvisioningJob";

	/// Run a job for the parent block indicated
	//
	// this function is in charge of creating and executing the job's main loop
	fn run(
		relay_parent: Hash,
		_run_args: Self::RunArgs,
		metrics: Self::Metrics,
		receiver: mpsc::Receiver<ToJob>,
		sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = ProvisioningJob::new(relay_parent, metrics, sender, receiver);

			// it isn't necessary to break run_loop into its own function,
			// but it's convenient to separate the concerns in this way
			job.run_loop().await
		}
		.boxed()
	}
}

impl ProvisioningJob {
	pub fn new(
		relay_parent: Hash,
		metrics: Metrics,
		sender: mpsc::Sender<FromJob>,
		receiver: mpsc::Receiver<ToJob>,
	) -> Self {
		Self {
			relay_parent,
			sender,
			receiver,
			provisionable_data_channels: Vec::new(),
			backed_candidates: Vec::new(),
			signed_bitfields: Vec::new(),
			metrics,
		}
	}

	async fn run_loop(mut self) -> Result<(), Error> {
		while let Some(msg) = self.receiver.next().await {
			use ProvisionerMessage::{
				ProvisionableData, RequestBlockAuthorshipData, RequestInherentData,
			};

			match msg {
				ToJob::Provisioner(RequestInherentData(_, return_sender)) => {
					if let Err(err) = send_inherent_data(
						self.relay_parent,
						&self.signed_bitfields,
						&self.backed_candidates,
						return_sender,
						self.sender.clone(),
					)
					.await
					{
						log::warn!(target: "provisioner", "failed to assemble or send inherent data: {:?}", err);
						self.metrics.on_inherent_data_request(Err(()));
					} else {
						self.metrics.on_inherent_data_request(Ok(()));
					}
				}
				ToJob::Provisioner(RequestBlockAuthorshipData(_, sender)) => {
					self.provisionable_data_channels.push(sender)
				}
				ToJob::Provisioner(ProvisionableData(_, data)) => {
					let mut bad_indices = Vec::new();
					for (idx, channel) in self.provisionable_data_channels.iter_mut().enumerate() {
						match channel.send(data.clone()).await {
							Ok(_) => {}
							Err(_) => bad_indices.push(idx),
						}
					}
					self.note_provisionable_data(data);

					// clean up our list of channels by removing the bad indices
					// start by reversing it for efficient pop
					bad_indices.reverse();
					// Vec::retain would be nicer here, but it doesn't provide
					// an easy API for retaining by index, so we re-collect instead.
					self.provisionable_data_channels = self
						.provisionable_data_channels
						.into_iter()
						.enumerate()
						.filter(|(idx, _)| {
							if bad_indices.is_empty() {
								return true;
							}
							let tail = bad_indices[bad_indices.len() - 1];
							let retain = *idx != tail;
							if *idx >= tail {
								let _ = bad_indices.pop();
							}
							retain
						})
						.map(|(_, item)| item)
						.collect();
				}
				ToJob::Stop => break,
			}
		}

		Ok(())
	}

	fn note_provisionable_data(&mut self, provisionable_data: ProvisionableData) {
		match provisionable_data {
			ProvisionableData::Bitfield(_, signed_bitfield) => {
				self.signed_bitfields.push(signed_bitfield)
			}
			ProvisionableData::BackedCandidate(backed_candidate) => {
				self.backed_candidates.push(backed_candidate)
			}
			_ => {}
		}
	}
}

type CoreAvailability = BitVec<bitvec::order::Lsb0, u8>;

/// The provisioner is the subsystem best suited to choosing which specific
/// backed candidates and availability bitfields should be assembled into the
/// block. To engage this functionality, a
/// `ProvisionerMessage::RequestInherentData` is sent; the response is a set of
/// non-conflicting candidates and the appropriate bitfields. Non-conflicting
/// means that there are never two distinct parachain candidates included for
/// the same parachain and that new parachain candidates cannot be included
/// until the previous one either gets declared available or expired.
///
/// The main complication here is going to be around handling
/// occupied-core-assumptions. We might have candidates that are only
/// includable when some bitfields are included. And we might have candidates
/// that are not includable when certain bitfields are included.
///
/// When we're choosing bitfields to include, the rule should be simple:
/// maximize availability. So basically, include all bitfields. And then
/// choose a coherent set of candidates along with that.
async fn send_inherent_data(
	relay_parent: Hash,
	bitfields: &[SignedAvailabilityBitfield],
	candidates: &[BackedCandidate],
	return_sender: oneshot::Sender<ProvisionerInherentData>,
	mut from_job: mpsc::Sender<FromJob>,
) -> Result<(), Error> {
	let availability_cores = request_availability_cores(relay_parent, &mut from_job)
		.await?
		.await??;

	let bitfields = select_availability_bitfields(&availability_cores, bitfields);
	let candidates = select_candidates(
		&availability_cores,
		&bitfields,
		candidates,
		relay_parent,
		&mut from_job,
	)
	.await?;

	return_sender
		.send((bitfields, candidates))
		.map_err(|_data| Error::InherentDataReturnChannel)?;
	Ok(())
}

/// In general, we want to pick all the bitfields. However, we have the following constraints:
///
/// - not more than one per validator
/// - each must correspond to an occupied core
///
/// If we have too many, an arbitrary selection policy is fine. For purposes of maximizing availability,
/// we pick the one with the greatest number of 1 bits.
///
/// Note: This does not enforce any sorting precondition on the output; the ordering there will be unrelated
/// to the sorting of the input.
fn select_availability_bitfields(
	cores: &[CoreState],
	bitfields: &[SignedAvailabilityBitfield],
) -> Vec<SignedAvailabilityBitfield> {
	let mut bitfield_per_core: Vec<Option<SignedAvailabilityBitfield>> = vec![None; cores.len()];
	let mut seen_validators = HashSet::new();

	for mut bitfield in bitfields.iter().cloned() {
		// If we have seen the validator already, ignore it.
		if !seen_validators.insert(bitfield.validator_index()) {
			continue;
		}

		for (idx, _) in cores.iter().enumerate().filter(|v| v.1.is_occupied()) {
			if *bitfield.payload().0.get(idx).unwrap_or(&false) {
				if let Some(ref mut occupied) = bitfield_per_core[idx] {
					if occupied.payload().0.count_ones() < bitfield.payload().0.count_ones() {
						// We found a better bitfield, lets swap them and search a new spot for the old
						// best one
						std::mem::swap(occupied, &mut bitfield);
					}
				} else {
					bitfield_per_core[idx] = Some(bitfield);
					break;
				}
			}
		}
	}

	bitfield_per_core.into_iter().filter_map(|v| v).collect()
}

/// Determine which cores are free, and then to the degree possible, pick a candidate appropriate to each free core.
async fn select_candidates(
	availability_cores: &[CoreState],
	bitfields: &[SignedAvailabilityBitfield],
	candidates: &[BackedCandidate],
	relay_parent: Hash,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<Vec<BackedCandidate>, Error> {
	let block_number = get_block_number_under_construction(relay_parent, sender).await?;

	let mut selected_candidates =
		Vec::with_capacity(candidates.len().min(availability_cores.len()));

	for (core_idx, core) in availability_cores.iter().enumerate() {
		let (scheduled_core, assumption) = match core {
			CoreState::Scheduled(scheduled_core) => (scheduled_core, OccupiedCoreAssumption::Free),
			CoreState::Occupied(occupied_core) => {
				if bitfields_indicate_availability(core_idx, bitfields, &occupied_core.availability) {
					if let Some(ref scheduled_core) = occupied_core.next_up_on_available {
						(scheduled_core, OccupiedCoreAssumption::Included)
					} else {
						continue;
					}
				} else {
					if occupied_core.time_out_at != block_number {
						continue;
					}
					if let Some(ref scheduled_core) = occupied_core.next_up_on_time_out {
						(scheduled_core, OccupiedCoreAssumption::TimedOut)
					} else {
						continue;
					}
				}
			}
			_ => continue,
		};

		let validation_data = match request_persisted_validation_data(
			relay_parent,
			scheduled_core.para_id,
			assumption,
			sender,
		)
		.await?
		.await??
		{
			Some(v) => v,
			None => continue,
		};

		let computed_validation_data_hash = validation_data.hash();

		// we arbitrarily pick the first of the backed candidates which match the appropriate selection criteria
		if let Some(candidate) = candidates.iter().find(|backed_candidate| {
			let descriptor = &backed_candidate.candidate.descriptor;
			descriptor.para_id == scheduled_core.para_id
				&& descriptor.persisted_validation_data_hash == computed_validation_data_hash
		}) {
			selected_candidates.push(candidate.clone());
		}
	}

	Ok(selected_candidates)
}

/// Produces a block number 1 higher than that of the relay parent
/// in the event of an invalid `relay_parent`, returns `Ok(0)`
async fn get_block_number_under_construction(
	relay_parent: Hash,
	sender: &mut mpsc::Sender<FromJob>,
) -> Result<BlockNumber, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send(FromJob::ChainApi(ChainApiMessage::BlockNumber(
			relay_parent,
			tx,
		)))
		.await
		.map_err(|e| Error::ChainApiMessageSend(e))?;
	match rx.await? {
		Ok(Some(n)) => Ok(n + 1),
		Ok(None) => Ok(0),
		Err(err) => Err(err.into()),
	}
}

/// The availability bitfield for a given core is the transpose
/// of a set of signed availability bitfields. It goes like this:
///
/// - construct a transverse slice along `core_idx`
/// - bitwise-or it with the availability slice
/// - count the 1 bits, compare to the total length; true on 2/3+
fn bitfields_indicate_availability(
	core_idx: usize,
	bitfields: &[SignedAvailabilityBitfield],
	availability: &CoreAvailability,
) -> bool {
	let mut availability = availability.clone();
	let availability_len = availability.len();

	for bitfield in bitfields {
		let validator_idx = bitfield.validator_index() as usize;
		match availability.get_mut(validator_idx) {
			None => {
				// in principle, this function might return a `Result<bool, Error>` so that we can more clearly express this error condition
				// however, in practice, that would just push off an error-handling routine which would look a whole lot like this one.
				// simpler to just handle the error internally here.
				log::warn!(
					target: "provisioner", "attempted to set a transverse bit at idx {} which is greater than bitfield size {}",
					validator_idx,
					availability_len,
				);

				return false;
			}
			Some(mut bit_mut) => *bit_mut |= bitfield.payload().0[core_idx],
		}
	}

	3 * availability.count_ones() >= 2 * availability.len()
}

#[derive(Clone)]
struct MetricsInner {
	inherent_data_requests: prometheus::CounterVec<prometheus::U64>,
}

/// Provisioner metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_inherent_data_request(&self, response: Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			match response {
				Ok(()) => metrics.inherent_data_requests.with_label_values(&["succeeded"]).inc(),
				Err(()) => metrics.inherent_data_requests.with_label_values(&["failed"]).inc(),
			}
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			inherent_data_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_inherent_data_requests_total",
						"Number of InherentData requests served by provisioner.",
					),
					&["success"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}


delegated_subsystem!(ProvisioningJob((), Metrics) <- ToJob as ProvisioningSubsystem);

#[cfg(test)]
mod tests;
