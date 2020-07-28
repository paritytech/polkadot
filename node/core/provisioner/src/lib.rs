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
	messages::{self, AllMessages, ProvisionableData, ProvisionerInherentData, ProvisionerMessage, RuntimeApiMessage},
	util::{JobTrait, ToJobTrait},
};
use polkadot_primitives::v1::{BackedCandidate, CoreState, Hash, SignedAvailabilityBitfield};
use std::{
	collections::HashSet,
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

#[derive(Debug, derive_more::From)]
pub enum Error {
	#[from]
	Sending(mpsc::SendError),
	OneshotReceiverDropped,
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
	pub fn new(relay_parent: Hash, sender: mpsc::Sender<FromJob>, receiver: mpsc::Receiver<ToJob>) -> Self {
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
			use ProvisionerMessage::{RequestBlockAuthorshipData, RequestInherentData, ProvisionableData};

			match msg {
				ToJob::Provisioner(RequestInherentData(_, sender)) => self.send_inherent_data(sender).await?,
				ToJob::Provisioner(RequestBlockAuthorshipData(_, sender)) => self.provisionable_data_channels.push(sender),
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

	// REVIEW: not async, but small; hopefully permissable
	fn note_provisionable_data(&mut self, provisionable_data: ProvisionableData) {
		use ProvisionableData::{Bitfield, BackedCandidate};

		match provisionable_data {
			Bitfield(_, signed_bitfield) => self.signed_bitfields.push(signed_bitfield),
			BackedCandidate(backed_candidate) => self.backed_candidates.push(backed_candidate),
			_ => {}
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
	async fn send_inherent_data(&mut self, sender: oneshot::Sender<ProvisionerInherentData>) -> Result<(), Error> {

		// coherency rules for backed candidates, recap:
		//
		// - [x] only one per parachain
		// - [ ] candidate is available (2/3+ of validators have set the 1 bit)
		// - [ ] not expired (how can we tell?)

		let availability_cores = self.get_availability_cores().await?;
		let mut coherent_candidates = Vec::new();
		let mut parachains_represented = HashSet::new();
		for candidate in self.backed_candidates.iter() {
			// only allow the first candidate per parachain
			// this sequence reliance is _entirely arbitrary_
			if !parachains_represented.insert(candidate.candidate.descriptor.para_id) {
				continue
			}

			// figure out which availability core corresponds to this candidate, then
			// update its `availability` bitfield with our list of bitfields to include.
			// then, if it's < 2/3 full, don't include it.

			unimplemented!()
		}

		// type ProvisionerInherentData = (Vec<SignedAvailabilityBitfield>, Vec<BackedCandidate>);
		sender.send((self.signed_bitfields.clone(), coherent_candidates)).map_err(|_| Error::OneshotReceiverDropped)?;
		Ok(())
	}

	async fn get_availability_cores(&mut self) -> Result<Vec<CoreState>, Error> {
		use RuntimeApiMessage::Request;
		// we need to bring this into this PR
		// use message::RuntimeApiRequest::AvailabilityCores;
		unimplemented!()
	}
}
