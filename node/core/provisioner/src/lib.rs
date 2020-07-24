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
	future::Either,
	prelude::*,
	select,
	stream::Stream,
	task,
};
use polkadot_node_subsystem::{
	messages::{AllMessages, ProvisionableData, ProvisionerInherentData, ProvisionerMessage},
	util::{JobTrait, ToJobTrait},
};
use polkadot_primitives::v1::{
	EncodeAs, Hash, HeadData, Id as ParaId, Signed, SigningContext, ValidatorId, ValidatorIndex,
	ValidatorPair,
};
use std::{
	convert::TryFrom,
	pin::Pin,
};

pub struct ProvisioningJob {
	receiver: mpsc::Receiver<ToJob>,
	provisionable_data_channels: Vec<mpsc::Sender<ProvisionableData>>,
	inherent_data_channels: Vec<mpsc::Sender<ProvisionerInherentData>>,
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

// not currently instantiable
pub enum FromJob {}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		unreachable!("uninstantiable; qed")
	}
}

#[derive(Debug, derive_more::From)]
pub enum Error {
	#[from]
	Sending(mpsc::SendError),
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
		_parent: Hash,
		_run_args: Self::RunArgs,
		receiver: mpsc::Receiver<ToJob>,
		_sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let job = ProvisioningJob::new(receiver);

			// it isn't necessary to break run_loop into its own function,
			// but it's convenient to separate the concerns in this way
			job.run_loop().await
		}
		.boxed()
	}
}

impl ProvisioningJob {
	pub fn new(receiver: mpsc::Receiver<ToJob>) -> Self {
		Self {
			receiver,
			provisionable_data_channels: Vec::new(),
			inherent_data_channels: Vec::new(),
		}
	}

	async fn run_loop(mut self) -> Result<(), Error> {
		while let Some(msg) = self.receiver.next().await {
			match msg {
				ToJob::Provisioner(_pm) => unimplemented!(),
				ToJob::Stop => break,
			}
		}

		Ok(())
	}
}
