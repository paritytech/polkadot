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

use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use polkadot_node_subsystem::{
	messages::{
		AllMessages, ProvisionableData, ProvisionerInherentData, ProvisionerMessage,
		RuntimeApiMessage,
	},
	util::{self, request_availability_cores, JobTrait, ToJobTrait},
};
use polkadot_primitives::v1::{BackedCandidate, CoreState, Hash, SignedAvailabilityBitfield};
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
	Runtime(RuntimeApiMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::Runtime(ram) => AllMessages::RuntimeApi(ram),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
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

		let (_, core) = match cores_by_id.get(&para_id) {
			Some(core) => core,
			None => return false,
		};

		// TODO: this is only a first draft; see https://github.com/paritytech/polkadot/pull/1473#discussion_r461845666
		std::matches!(core, CoreState::Scheduled(_))
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
