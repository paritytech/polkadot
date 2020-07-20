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

use futures::{
	channel::{mpsc, oneshot},
	future::{abortable, AbortHandle},
	prelude::*,
	Future,
};
use polkadot_node_subsystem::{
	messages::{AllMessages, BitfieldSigningMessage},
	OverseerSignal, SubsystemResult,
};
use polkadot_node_subsystem::{FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext};
use polkadot_primitives::Hash;
use std::{
	collections::HashMap,
	pin::Pin,
	time::{Duration, Instant},
};

/// Delay between starting a bitfield signing job and its attempting to create a bitfield.
const JOB_DELAY: Duration = Duration::from_millis(1500);

/// JobCanceler aborts all abort handles on drop.
#[derive(Debug, Default)]
struct JobCanceler(HashMap<Hash, AbortHandle>);

// AbortHandle doesn't impl Drop on its own, so we wrap it
// in this struct to get free cancellation on drop.
impl Drop for JobCanceler {
	fn drop(&mut self) {
		for abort_handle in self.0.values() {
			abort_handle.abort();
		}
	}
}

/// Bitfield signing subsystem.
struct BitfieldSigning;

impl BitfieldSigning {
	async fn run<Context>(mut ctx: Context) -> SubsystemResult<()>
	where
		Context: SubsystemContext<Message = BitfieldSigningMessage> + Clone,
	{
		let mut active_jobs = JobCanceler::default();

		loop {
			use FromOverseer::*;
			use OverseerSignal::*;
			match ctx.recv().await {
				Ok(Communication { msg: _ }) => {
					unreachable!("BitfieldSigningMessage is uninstantiable; qed")
				}
				Ok(Signal(StartWork(hash))) => {
					let (future, abort_handle) =
						abortable(bitfield_signing_job(hash.clone(), ctx.clone()));
					// future currently returns a Result based on whether or not it was aborted;
					// let's ignore all that and return () unconditionally, to fit the interface.
					let future = async move {
						let _ = future.await;
					};
					active_jobs.0.insert(hash.clone(), abort_handle);
					ctx.spawn(Box::pin(future)).await?;
				}
				Ok(Signal(StopWork(hash))) => {
					if let Some(abort_handle) = active_jobs.0.remove(&hash) {
						abort_handle.abort();
					}
				}
				Ok(Signal(Conclude)) => break,
				Err(err) => {
					return Err(err);
				}
			}
		}

		Ok(())
	}
}

impl<Context> Subsystem<Context> for BitfieldSigning
where
	Context: SubsystemContext<Message = BitfieldSigningMessage> + Clone,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			if let Err(err) = Self::run(ctx).await {
				log::error!("{:?}", err);
			};
		}))
	}
}

async fn bitfield_signing_job<Context>(hash: Hash, ctx: Context)
where
	Context: SubsystemContext<Message = BitfieldSigningMessage>,
{
	// first up, figure out when we need to wait until
	let delay = wasm_timer::Delay::new_at(Instant::now() + JOB_DELAY);
	// next, do some prerequisite work
	todo!();
	// now, wait for the delay to be complete
	if let Err(_) = delay.await {
		return;
	}
	// let (tx, _) = oneshot::channel();

	// ctx.send_message(AllMessages::CandidateValidation(
	// 	CandidateValidationMessage::Validate(
	// 		Default::default(),
	// 		Default::default(),
	// 		PoVBlock {
	// 			block_data: BlockData(Vec::new()),
	// 		},
	// 		tx,
	// 	)
	// )).await.unwrap();
	unimplemented!()
}
