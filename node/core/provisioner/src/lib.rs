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

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use polkadot_node_subsystem::{
	errors::ChainApiError,
	messages::{
		AllMessages, ChainApiMessage, ProvisionableData, ProvisionerInherentData, ProvisionerMessage,
		RuntimeApiMessage,
	},
	util::{self, request_availability_cores, JobTrait, ToJobTrait},
};
use polkadot_primitives::v1::{BackedCandidate, BlockNumber, CoreState, Hash, SignedAvailabilityBitfield};
use std::{
	collections::{HashMap, HashSet},
	convert::TryFrom,
	pin::Pin,
};

pub struct ProvisioningJob {
	relay_parent: Hash,
	sender: mpsc::Sender<FromJob>,
	receiver: mpsc::Receiver<ToJob>,
	provisionable_data_channels: Vec<mpsc::Sender<ProvisionableData>>,
	backed_candidates: Vec<BackedCandidate>,
	signed_bitfields: Vec<SignedAvailabilityBitfield>,
}

pub enum ToJob {
	Provisioner(ProvisionerMessage),
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

pub enum FromJob {
	ChainApi(ChainApiMessage),
	Runtime(RuntimeApiMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::ChainApi(cam) => unimplemented!("ChainApiMessage is not a variant of AllMessages yet"),
			FromJob::Runtime(ram) => AllMessages::RuntimeApi(ram),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			// TODO: resolve this as well as the above unimplemented! section
			// AllMessages::ChainApi(chain) => Ok(FromJob::ChainApi(chain)),
			AllMessages::RuntimeApi(runtime) => Ok(FromJob::Runtime(runtime)),
			_ => Err(()),
		}
	}
}

#[derive(Debug, derive_more::From)]
pub enum Error {
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
					// Note the cloning here: we have to clone the vectors of signed bitfields and backed candidates
					// so that we can respond to more than a single request for inherent data; we can't just move them.
					// It would be legal, however, to set up `from_job` as `&mut _` instead of a clone. We clone it instead
					// of borrowing it so that this async function doesn't depend at all on the lifetime of `&self`.
					send_inherent_data(
						self.relay_parent,
						self.signed_bitfields.clone(),
						self.backed_candidates.clone(),
						sender,
						self.sender.clone(),
					)
					.await?
				}
				ToJob::Provisioner(RequestBlockAuthorshipData(_, sender)) => {
					self.provisionable_data_channels.push(sender)
				}
				ToJob::Provisioner(ProvisionableData(data)) => {
					for channel in self.provisionable_data_channels.iter_mut() {
						// REVIEW: the try operator here breaks the run loop if any receiver ever unexpectedly
						// closes their channel. Is that desired?
						channel.send(data.clone()).await?;
					}
					self.note_provisionable_data(data);
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
	signed_bitfields: Vec<SignedAvailabilityBitfield>,
	mut backed_candidates: Vec<BackedCandidate>,
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

	// availability cores indexed by their para_id for ease of use
	let cores_by_id: HashMap<_, _> = availability_cores
		.iter()
		.enumerate()
		.filter_map(|(idx, core_state)| Some((core_state.para_id()?, (idx, core_state))))
		.collect();

	// utility
	let mut parachains_represented = HashSet::new();

	// ideally, we wouldn't calculate this unconditionally, but only if we have to use it in the
	// `if let Some(ref scheduled) = occupied.next_up_on_time_out {` case. In all other cases, it's
	// irrelevant. However, that's challenging: that whole thing happens within a `.retain` closure,
	// which is _not_ async.
	// We can leave the optimization for another day.
	let block_number = match get_block_number_under_construction(relay_parent, &mut from_job).await {
		Ok(n) => n,
		Err(err) => {
			log::warn!(target: "Provisioner", "failed to get number of block under construction: {:?}", err);
			0
		}
	};

	// coherent candidates fulfill these conditions:
	//
	// - only one per parachain
	// - any of:
	//   - this para is assigned to a `Scheduled` core
	//   - this para is assigned to an `Occupied` core, and any of:
	//     - it is `next_up_on_available` and the bitfields we are including, merged with
	//       the `availability` vec, form 2/3+ of validators
	//     - it is `next_up_on_time_out` and the bitfields we are including, merged with
	//       the `availability_ vec, for <2/3 of validators, and `time_out_at` is
	//       the block we are building.
	//
	// postcondition: they are sorted by core index
	backed_candidates.retain(|candidate| {
		// only allow the first candidate per parachain
		let para_id = candidate.candidate.descriptor.para_id;
		if !parachains_represented.insert(para_id) {
			return false;
		}

		let (core_idx, core) = match cores_by_id.get(&para_id) {
			Some(core) => core,
			None => return false,
		};

		match core {
			CoreState::Free => false,
			CoreState::Scheduled(_) => true,
			CoreState::Occupied(occupied) => {
				if let Some(ref scheduled) = occupied.next_up_on_available {
					return scheduled.para_id == para_id
						&& merged_bitfields_are_gte_two_thirds(
							*core_idx,
							&signed_bitfields,
							&occupied.availability,
						);
				}
				if let Some(ref scheduled) = occupied.next_up_on_time_out {
					return scheduled.para_id == para_id
						&& occupied.time_out_at == block_number
						&& !merged_bitfields_are_gte_two_thirds(
							*core_idx,
							&signed_bitfields,
							&occupied.availability,
						);
				}
				false
			}
		}
	});

	// ensure the postcondition holds true: sorted by core index
	// note: unstable sort is still deterministic becuase we know (by means of `parachains_represented`) that
	// no two backed candidates remain, both of which are assigned to the same core.
	backed_candidates.sort_unstable_by_key(|candidate| {
		let para_id = candidate.candidate.descriptor.para_id;
		let (core_idx, _) = cores_by_id.get(&para_id).expect("paras not assigned to a core have already been eliminated from the backed_candidates list; qed");
		core_idx
	});

	// type ProvisionerInherentData = (Vec<SignedAvailabilityBitfield>, Vec<BackedCandidate>);
	return_sender
		.send((signed_bitfields, backed_candidates))
		.map_err(|_| Error::OneshotSend)?;
	Ok(())
}

// produces a block number 1 higher than that of the relay parent
// in the event of an invalid `relay_parent`, returns `Ok(0)`
async fn get_block_number_under_construction(relay_parent: Hash, sender: &mut mpsc::Sender<FromJob>) -> Result<BlockNumber, Error> {
	let (tx, rx) = oneshot::channel();
	sender.send(FromJob::ChainApi(ChainApiMessage::BlockNumber(relay_parent, tx))).await.map_err(|_| Error::OneshotSend)?;
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
	availability: &BitVec<bitvec::order::Lsb0, u8>,
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
