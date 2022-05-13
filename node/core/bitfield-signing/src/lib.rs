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
#![recursion_limit = "256"]

use futures::{
	channel::{mpsc, oneshot},
	future,
	lock::Mutex,
	prelude::*,
	Future,
};
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	jaeger,
	messages::{
		AvailabilityStoreMessage, BitfieldDistributionMessage, BitfieldSigningMessage,
		RuntimeApiMessage, RuntimeApiRequest,
	},
	overseer, ActivatedLeaf, LeafStatus, PerLeafSpan, SubsystemSender,
};
use polkadot_node_subsystem_util::{self as util, JobSender, JobSubsystem, JobTrait, Validator};
use polkadot_primitives::v2::{AvailabilityBitfield, CoreState, Hash, ValidatorIndex};
use sp_keystore::{Error as KeystoreError, SyncCryptoStorePtr};
use std::{iter::FromIterator, pin::Pin, time::Duration};
use wasm_timer::{Delay, Instant};

mod metrics;
use self::metrics::Metrics;

#[cfg(test)]
mod tests;

/// Delay between starting a bitfield signing job and its attempting to create a bitfield.
const JOB_DELAY: Duration = Duration::from_millis(1500);
const LOG_TARGET: &str = "parachain::bitfield-signing";

/// Each `BitfieldSigningJob` prepares a signed bitfield for a single relay parent.
pub struct BitfieldSigningJob<Sender>(std::marker::PhantomData<Sender>);

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
async fn get_core_availability(
	core: &CoreState,
	validator_idx: ValidatorIndex,
	sender: &Mutex<&mut impl SubsystemSender<overseer::BitfieldSigningOutgoingMessages>>,
	span: &jaeger::Span,
) -> Result<bool, Error> {
	if let &CoreState::Occupied(ref core) = core {
		let _span = span.child("query-chunk-availability");

		let (tx, rx) = oneshot::channel();
		sender
			.lock()
			.await
			.send_message(
				AvailabilityStoreMessage::QueryChunkAvailability(
					core.candidate_hash,
					validator_idx,
					tx,
				)
				.into(),
			)
			.await;

		let res = rx.await.map_err(Into::into);

		gum::trace!(
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
	sender: &mut impl SubsystemSender<overseer::BitfieldSigningOutgoingMessages>,
) -> Result<Vec<CoreState>, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(
			RuntimeApiMessage::Request(relay_parent, RuntimeApiRequest::AvailabilityCores(tx))
				.into(),
		)
		.await;
	match rx.await {
		Ok(Ok(out)) => Ok(out),
		Ok(Err(runtime_err)) => Err(runtime_err.into()),
		Err(err) => Err(err.into()),
	}
}

/// - get the list of core states from the runtime
/// - for each core, concurrently determine chunk availability (see `get_core_availability`)
/// - return the bitfield if there were no errors at any point in this process
///   (otherwise, it's prone to false negatives)
async fn construct_availability_bitfield(
	relay_parent: Hash,
	span: &jaeger::Span,
	validator_idx: ValidatorIndex,
	sender: &mut impl SubsystemSender<overseer::BitfieldSigningOutgoingMessages>,
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
		availability_cores
			.iter()
			.map(|core| get_core_availability(core, validator_idx, &sender, span)),
	)
	.await?;

	let core_bits = FromIterator::from_iter(results.into_iter());
	gum::debug!(
		target: LOG_TARGET,
		?relay_parent,
		"Signing Bitfield for {core_count} cores: {core_bits}",
		core_count = availability_cores.len(),
		core_bits = core_bits,
	);

	Ok(AvailabilityBitfield(core_bits))
}

impl<Sender> JobTrait for BitfieldSigningJob<Sender>
where
	Sender: overseer::BitfieldSigningSenderTrait + Unpin,
{
	type ToJob = BitfieldSigningMessage;
	type OutgoingMessages = overseer::BitfieldSigningOutgoingMessages;
	type Sender = Sender;
	type Error = Error;
	type RunArgs = SyncCryptoStorePtr;
	type Metrics = Metrics;

	const NAME: &'static str = "bitfield-signing-job";

	/// Run a job for the parent block indicated
	fn run(
		leaf: ActivatedLeaf,
		keystore: Self::RunArgs,
		metrics: Self::Metrics,
		_receiver: mpsc::Receiver<BitfieldSigningMessage>,
		mut sender: JobSender<Sender>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		let metrics = metrics.clone();
		async move {
			if let LeafStatus::Stale = leaf.status {
				gum::debug!(
					target: LOG_TARGET,
					hash = ?leaf.hash,
					block_number =  ?leaf.number,
					"Stale leaf - don't sign bitfields."
				);
				return Ok(())
			}

			let span = PerLeafSpan::new(leaf.span, "bitfield-signing");
			let _span = span.child("delay");
			let wait_until = Instant::now() + JOB_DELAY;

			// now do all the work we can before we need to wait for the availability store
			// if we're not a validator, we can just succeed effortlessly
			let validator = match Validator::new(leaf.hash, keystore.clone(), &mut sender).await {
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

			let bitfield = match construct_availability_bitfield(
				leaf.hash,
				&span_availability,
				validator.index(),
				sender.subsystem_sender(),
			)
			.await
			{
				Err(Error::Runtime(runtime_err)) => {
					// Don't take down the node on runtime API errors.
					gum::warn!(target: LOG_TARGET, err = ?runtime_err, "Encountered a runtime API error");
					return Ok(())
				},
				Err(err) => return Err(err),
				Ok(bitfield) => bitfield,
			};

			drop(span_availability);
			let _span = span.child("signing");

			let signed_bitfield = match validator
				.sign(keystore.clone(), bitfield)
				.await
				.map_err(|e| Error::Keystore(e))?
			{
				Some(b) => b,
				None => {
					gum::error!(
						target: LOG_TARGET,
						"Key was found at construction, but while signing it could not be found.",
					);
					return Ok(())
				},
			};

			metrics.on_bitfield_signed();

			drop(_span);
			let _span = span.child("gossip");

			sender
				.send_message(BitfieldDistributionMessage::DistributeBitfield(
					leaf.hash,
					signed_bitfield,
				))
				.await;

			Ok(())
		}
		.boxed()
	}
}

/// `BitfieldSigningSubsystem` manages a number of bitfield signing jobs.
pub type BitfieldSigningSubsystem<Spawner, Sender> =
	JobSubsystem<BitfieldSigningJob<Sender>, Spawner>;
