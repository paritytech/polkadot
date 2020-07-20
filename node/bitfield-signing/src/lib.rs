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

use bitvec::bitvec;
use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
	Future,
};
use keystore::KeyStorePtr;
use polkadot_node_subsystem::{
	messages::{self, AllMessages, AvailabilityStoreMessage, BitfieldDistributionMessage, BitfieldSigningMessage, CandidateBackingMessage},
	util::{self, JobManager, JobTrait, ToJobTrait, Validator},
};
use polkadot_primitives::v1::{
	BackedCandidate, Hash,
};
use std::{
	convert::TryFrom,
	pin::Pin,
	time::Duration,
};
use wasm_timer::{Delay, Instant};

/// Delay between starting a bitfield signing job and its attempting to create a bitfield.
const JOB_DELAY: Duration = Duration::from_millis(1500);

/// Each `BitfieldSigningJob` prepares a signed bitfield for a single relay parent.
pub struct BitfieldSigningJob;

/// Messages which a `BitfieldSigningJob` is prepared to receive.
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
			_ => Err(())
		}
	}
}

impl From<BitfieldSigningMessage> for ToJob {
	fn from(bsm: BitfieldSigningMessage) -> ToJob {
		ToJob::BitfieldSigning(bsm)
	}
}

/// Messages which may be sent from a `BitfieldSigningJob`.
pub enum FromJob {
	AvailabilityStore(AvailabilityStoreMessage),
	BitfieldDistribution(BitfieldDistributionMessage),
	CandidateBacking(CandidateBackingMessage),
}

impl From<FromJob> for AllMessages {
	fn from(from_job: FromJob) -> AllMessages {
		match from_job {
			FromJob::AvailabilityStore(asm) => AllMessages::AvailabilityStore(asm),
			FromJob::BitfieldDistribution(bdm) => AllMessages::BitfieldDistribution(bdm),
			FromJob::CandidateBacking(cbm) => AllMessages::CandidateBacking(cbm),
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
			_ => Err(()),
		}
	}
}

/// Errors we may encounter in the course of executing the `BitfieldSigningSubsystem`.
#[derive(Debug, derive_more::From)]
pub enum Error {
	/// error propagated from the utility subsystem
	#[from]
	Util(util::Error),
	/// io error
	#[from]
	Io(std::io::Error),
	/// a one shot channel was canceled
	#[from]
	Oneshot(oneshot::Canceled),
	/// a mspc channel failed to send
	#[from]
	MpscSend(mpsc::SendError),
}

async fn get_backed_candidates_pending_availability(relay_parent: Hash, sender: &mut mpsc::Sender<FromJob>) -> Result<Vec<BackedCandidate>, Error> {
	use FromJob::CandidateBacking;
	use CandidateBackingMessage::GetBackedCandidates;
	use messages::NewBackedCandidate;

	let (tx, rx) = oneshot::channel();
	// REVIEW: is this where we should be getting the set of backed candidates from?
	sender.send(CandidateBacking(GetBackedCandidates(relay_parent, tx))).await?;
	Ok(rx.await?.into_iter().map(|NewBackedCandidate(backed_candidate)| backed_candidate).collect())
}

impl JobTrait for BitfieldSigningJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = KeyStorePtr;

	const NAME: &'static str = "BitfieldSigningJob";

	/// Run a job for the parent block indicated
	fn run(
		parent: Hash,
		keystore: Self::RunArgs,
		_receiver: mpsc::Receiver<ToJob>,
		mut sender: mpsc::Sender<FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			// figure out when to wait to
			let wait_until = Instant::now() + JOB_DELAY;

			// now do all the work we can before we need to wait for the availability store
			// if we're not a validator, we can just succeed effortlessly
			let validator = match Validator::new(parent, keystore, sender.clone()).await {
				Ok(validator) => validator,
				Err(util::Error::NotAValidator) => return Ok(()),
				Err(err) => return Err(Error::Util(err)),
			};

			// wait a bit before doing anything else
			Delay::new_at(wait_until).await?;

			let backed_candidates = get_backed_candidates_pending_availability(parent, &mut sender).await?;
			let bitvec_size: usize = todo!();
			let mut out = bitvec![0; bitvec_size];

			unimplemented!()
		}.boxed()
	}
}

/// BitfieldSigningSubsystem manages a number of bitfield signing jobs.
pub type BitfieldSigningSubsystem<Spawner, Context> = JobManager<Spawner, Context, BitfieldSigningJob>;
