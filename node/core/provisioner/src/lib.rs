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

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use polkadot_node_subsystem::{
	delegated_subsystem,
	errors::ChainApiError,
	messages::{
		AllMessages, ChainApiMessage, ProvisionableData, ProvisionerInherentData,
		ProvisionerMessage, RuntimeApiMessage,
	},
	util::{self, request_availability_cores, request_global_validation_data, request_local_validation_data, JobTrait, ToJobTrait},
};
use polkadot_primitives::v1::{
	BackedCandidate, BlockNumber, CoreState, GlobalValidationData, LocalValidationData,Hash, OccupiedCore, OccupiedCoreAssumption,
	ScheduledCore, SignedAvailabilityBitfield,
};
use std::{collections::HashMap, convert::TryFrom, pin::Pin};

struct ProvisioningJob {
	relay_parent: Hash,
	sender: mpsc::Sender<FromJob>,
	receiver: mpsc::Receiver<ToJob>,
	provisionable_data_channels: Vec<mpsc::Sender<ProvisionableData>>,
	backed_candidates: Vec<BackedCandidate>,
	signed_bitfields: Vec<SignedAvailabilityBitfield>,
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
	OneshotSend,
}

impl JobTrait for ProvisioningJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = ();

	const NAME: &'static str = "ProvisioningJob";

	/// Run a job for the parent block indicated
	//
	// this function is in charge of creating and executing the job's main loop
	fn run(
		relay_parent: Hash,
		_run_args: Self::RunArgs,
		receiver: mpsc::Receiver<ToJob>,
		sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = ProvisioningJob::new(relay_parent, sender, receiver);

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
		}
	}

	async fn run_loop(mut self) -> Result<(), Error> {
		while let Some(msg) = self.receiver.next().await {
			use ProvisionerMessage::{
				ProvisionableData, RequestBlockAuthorshipData, RequestInherentData,
			};

			match msg {
				ToJob::Provisioner(RequestInherentData(_, sender)) => {
					send_inherent_data(
						self.relay_parent,
						&self.signed_bitfields,
						&self.backed_candidates,
						sender,
						self.sender.clone(),
					)
					.await?
				}
				ToJob::Provisioner(RequestBlockAuthorshipData(_, sender)) => {
					self.provisionable_data_channels.push(sender)
				}
				ToJob::Provisioner(ProvisionableData(data)) => {
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
								bad_indices.pop();
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

// preprocessing the cores involves a bit more data than is comfortable in a tuple, so let's make a struct of it
struct PreprocessedCore {
	relay_parent: Hash,
	assumption: OccupiedCoreAssumption,
	scheduled_core: ScheduledCore,
	availability: Option<CoreAvailability>,
	timeout: Option<BlockNumber>,
	idx: usize,
}

impl PreprocessedCore {
	fn new(relay_parent: Hash, idx: usize, core: CoreState) -> Option<Self> {
		match core {
			CoreState::Occupied(OccupiedCore {
				availability,
				next_up_on_available: Some(scheduled_core),
				..
			}) => Some(Self {
				relay_parent,
				assumption: OccupiedCoreAssumption::Included,
				scheduled_core,
				availability: Some(availability),
				timeout: None,
				idx,
			}),
			CoreState::Occupied(OccupiedCore {
				availability,
				next_up_on_time_out: Some(scheduled_core),
				time_out_at,
				..
			}) => Some(Self {
				relay_parent,
				assumption: OccupiedCoreAssumption::TimedOut,
				scheduled_core,
				availability: Some(availability),
				timeout: Some(time_out_at),
				idx,
			}),
			CoreState::Scheduled(scheduled_core) => Some(Self {
				relay_parent,
				assumption: OccupiedCoreAssumption::Free,
				scheduled_core,
				availability: None,
				timeout: None,
				idx,
			}),
			_ => None,
		}
	}

	// coherent candidates fulfill these conditions:
	//
	// - only one per parachain
	// - any of:
	//   - this para is assigned to a `Scheduled` core (OccupiedCoreAssumption::Free)
	//   - this para is assigned to an `Occupied` core, and any of:
	//     - it is `next_up_on_available` (OccupiedCoreAssumption::Included),
	//       and the bitfields we are including, merged with the `availability` vec, form 2/3+ of validators
	//     - it is `next_up_on_time_out` (OccupiedCoreAssumption::TimedOut),
	//       and `time_out_at` is the block we are building,
	//       and the bitfields we are including, merged with the `availability_ vec, form <2/3 of validators
	fn choose_candidate(
		&self,
		bitfields: &[SignedAvailabilityBitfield],
		candidates: &[BackedCandidate],
		block_number: BlockNumber,
		sender: &mut mpsc::Sender<FromJob>,
	) -> Option<BackedCandidate> {
		// the validation data hash must match under the appropriate occupied core assumption.
		// to compute the validation data hash, we need both global and local validation data.
		let global_validation_data = request_global_validation_data(self.relay_parent, sender).await.ok()?.await.ok()?.ok()?;

		// choose only one per parachain
		candidates
			.iter()
			.find(|candidate| candidate.candidate.descriptor.para_id == self.scheduled_core.para_id)
			.filter(|candidate| {
				let local_validation_data = request_local_validation_data(self.relay_parent, self.scheduled_core.para_id, self.assumption).await.ok()?.await.ok()?.ok()?;

			})
			.map(|candidate| {
				match (self.assumption, self.availability.as_ref(), self.timeout) {
					(OccupiedCoreAssumption::Free, _, _) => {
						// core was already scheduled
						Some(candidate.clone())
					}
					(OccupiedCoreAssumption::Included, Some(availability), _) => {
						// core became available
						if merged_bitfields_are_gte_two_thirds(self.idx, &bitfields, availability) {
							Some(candidate.clone())
						} else {
							None
						}
					}
					(OccupiedCoreAssumption::TimedOut, Some(availability), Some(timeout)) => {
						// core timed out
						if timeout == block_number
							&& !merged_bitfields_are_gte_two_thirds(
								self.idx,
								&bitfields,
								availability,
							)
						{
							Some(candidate.clone())
						} else {
							None
						}
					}
					_ => None,
				}
			})
			.flatten()
	}
}

// The provisioner is the subsystem best suited to choosing which specific
// backed candidates and availability bitfields should be assembled into the
// block. To engage this functionality, a
// `ProvisionerMessage::RequestInherentData` is sent; the response is a set of
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
async fn send_inherent_data(
	relay_parent: Hash,
	signed_bitfields: &[SignedAvailabilityBitfield],
	backed_candidates: &[BackedCandidate],
	return_sender: oneshot::Sender<ProvisionerInherentData>,
	mut from_job: mpsc::Sender<FromJob>,
) -> Result<(), Error> {
	let availability_cores = match request_availability_cores(relay_parent, &mut from_job)
		.await?
		.await?
	{
		Ok(cores) => cores,
		Err(runtime_err) => {
			// Don't take down the node on runtime API errors.
			log::warn!(target: "provisioner", "Encountered a runtime API error: {:?}", runtime_err);
			return Ok(());
		}
	};

	// select those bitfields which match our constraints
	let signed_bitfields = select_availability_bitfields(&availability_cores, signed_bitfields);

	// preprocess the availability cores: replace occupied cores with scheduled cores, if possible
	// also tag each core with an `OccupiedCoreAssumption`
	let scheduled_cores: Vec<_> = availability_cores
		.into_iter()
		.enumerate()
		.filter_map(|(idx, core)| PreprocessedCore::new(idx, core))
		.collect();

	let block_number = match get_block_number_under_construction(relay_parent, &mut from_job).await
	{
		Ok(n) => n,
		Err(err) => {
			log::warn!(target: "Provisioner", "failed to get number of block under construction: {:?}", err);
			0
		}
	};

	// postcondition: they are sorted by core index, but that's free, since we're iterating in order of cores anyway
	let selected_candidates: Vec<_> = scheduled_cores
		.into_iter()
		.filter_map(|core| {
			core.choose_candidate(&signed_bitfields, backed_candidates, block_number)
		})
		.collect();

	// type ProvisionerInherentData = (Vec<SignedAvailabilityBitfield>, Vec<BackedCandidate>);
	return_sender
		.send((signed_bitfields, selected_candidates))
		.map_err(|_| Error::OneshotSend)?;
	Ok(())
}

// in general, we want to pick all the bitfields. However, we have the following constraints:
//
// - not more than one per validator
// - each must correspond to an occupied core
//
// If we have too many, an arbitrary selection policy is fine. For purposes of maximizing availability,
// we pick the one with the greatest number of 1 bits.
//
// note: this does not enforce any sorting precondition on the output; the ordering there will be unrelated
// to the sorting of the input.
fn select_availability_bitfields(
	cores: &[CoreState],
	bitfields: &[SignedAvailabilityBitfield],
) -> Vec<SignedAvailabilityBitfield> {
	let mut fields_by_core: HashMap<_, Vec<_>> = HashMap::new();
	for bitfield in bitfields.iter() {
		let core_idx = bitfield.validator_index() as usize;
		if let CoreState::Occupied(_) = cores[core_idx] {
			fields_by_core
				.entry(core_idx)
				.or_default()
				.push(bitfield.clone());
		}
	}

	// there cannot be a value list in field_by_core with len < 1
	let mut out = Vec::with_capacity(fields_by_core.len());
	for (_, core_bitfields) in fields_by_core.iter_mut() {
		core_bitfields.sort_by_key(|bitfield| bitfield.payload().0.count_ones());
		out.push(
			core_bitfields
				.pop()
				.expect("every core bitfield has at least 1 member; qed"),
		);
	}

	out
}

// produces a block number 1 higher than that of the relay parent
// in the event of an invalid `relay_parent`, returns `Ok(0)`
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
		.map_err(|_| Error::OneshotSend)?;
	match rx.await? {
		Ok(Some(n)) => Ok(n + 1),
		Ok(None) => Ok(0),
		Err(err) => Err(err.into()),
	}
}

// The instructions state:
//
// > we can only include the candidate if the bitfields we are including _and_ the availability vec of the OccupiedCore
//
// The natural implementation takes advantage of the fact that the availability bitfield for a given core is the transpose
// of a set of signed availability bitfields. It goes like this:
//
//   - organize the incoming bitfields by validator index
//   - construct a transverse slice along `core_idx`
//   - bitwise-or it with the availability slice
//   - count the 1 bits, compare to the total length
fn merged_bitfields_are_gte_two_thirds(
	core_idx: usize,
	bitfields: &[SignedAvailabilityBitfield],
	availability: &CoreAvailability,
) -> bool {
	let mut transverse = availability.clone();
	let transverse_len = transverse.len();

	for bitfield in bitfields {
		let validator_idx = bitfield.validator_index() as usize;
		match transverse.get_mut(validator_idx) {
			None => {
				// in principle, this function might return a `Result<bool, Error>` so that we can more clearly express this error condition
				// however, in practice, that would just push off an error-handling routine which would look a whole lot like this one.
				// simpler to just handle the error internally here.
				log::warn!(target: "provisioner", "attempted to set a transverse bit at idx {} which is greater than bitfield size {}", validator_idx, transverse_len);
				return false;
			}
			Some(mut bit_mut) => *bit_mut |= bitfield.payload().0[core_idx],
		}
	}
	3 * transverse.count_ones() >= 2 * transverse.len()
}

delegated_subsystem!(ProvisioningJob(()) <- ToJob as ProvisioningSubsystem);

#[cfg(test)]
mod tests {
	mod select_availability_bitfields {
		use bitvec::bitvec;
		use polkadot_primitives::v1::{
			GroupIndex, Id as ParaId, ValidatorPair
		};
		use sp_core::crypto::Pair;
		use super::super::*;

		fn occupied_core(para_id: ParaId, group_responsible: GroupIndex) -> CoreState {
			CoreState::Occupied(OccupiedCore {
				para_id,
				group_responsible,
				next_up_on_available: None,
				occupied_since: 100_u32,
				time_out_at: 200_u32,
				next_up_on_time_out: None,
				availability: bitvec![bitvec::order::Lsb0, u8; 0; 32],
			})
		}

		fn signed_bitfield(field: CoreAvailability, validator: &ValidatorPair, ) -> SignedAvailabilityBitfield {
			unimplemented!()
		}

		#[test]
		fn not_more_than_one_per_validator() {
			let validator = ValidatorPair::generate().0;
			let cores = vec![occupied_core(0.into(), 0.into()), occupied_core(1.into(), 1.into())];
			let bitfields = vec![];
			unimplemented!()
		}

		#[test]
		fn each_corresponds_to_an_occupied_core() {
			unimplemented!()
		}
	}
}
