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

//! Shows a basic usage of the `Overseer`:
//!   * Spawning subsystems and subsystem child jobs
//!   * Establishing message passing

use futures::{channel::oneshot, pending, pin_mut, select, stream, FutureExt, StreamExt};
use futures_timer::Delay;
use orchestra::async_trait;
use std::time::Duration;

use ::test_helpers::{dummy_candidate_descriptor, dummy_hash};
use polkadot_node_primitives::{BlockData, PoV};
use polkadot_node_subsystem_types::messages::CandidateValidationMessage;
use polkadot_overseer::{
	self as overseer,
	dummy::dummy_overseer_builder,
	gen::{FromOrchestra, SpawnedSubsystem},
	HeadSupportsParachains, SubsystemError,
};
use polkadot_primitives::v2::{CandidateReceipt, Hash};

struct AlwaysSupportsParachains;

#[async_trait]
impl HeadSupportsParachains for AlwaysSupportsParachains {
	async fn head_supports_parachains(&self, _head: &Hash) -> bool {
		true
	}
}

////////

struct Subsystem1;

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
impl Subsystem1 {
	async fn run<Context>(mut ctx: Context) {
		'louy: loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					if let FromOrchestra::Communication { msg } = msg {
						gum::info!("msg {:?}", msg);
					}
					continue 'louy
				},
				Ok(None) => (),
				Err(_) => {
					gum::info!("exiting");
					break 'louy
				},
			}

			Delay::new(Duration::from_secs(1)).await;
			let (tx, _) = oneshot::channel();

			let candidate_receipt = CandidateReceipt {
				descriptor: dummy_candidate_descriptor(dummy_hash()),
				commitments_hash: Hash::zero(),
			};

			let msg = CandidateValidationMessage::ValidateFromChainState(
				candidate_receipt,
				PoV { block_data: BlockData(Vec::new()) }.into(),
				Default::default(),
				tx,
			);
			ctx.send_message(msg).await;
		}
		()
	}
}

#[overseer::subsystem(CandidateBacking, error = SubsystemError, prefix = self::overseer)]
impl<Context> Subsystem1 {
	fn start(self, ctx: Context) -> SpawnedSubsystem<SubsystemError> {
		let future = Box::pin(async move {
			Self::run(ctx).await;
			Ok(())
		});

		SpawnedSubsystem { name: "subsystem-1", future }
	}
}

//////////////////

struct Subsystem2;

#[overseer::contextbounds(CandidateValidation, prefix = self::overseer)]
impl Subsystem2 {
	async fn run<Context>(mut ctx: Context) -> () {
		ctx.spawn(
			"subsystem-2-job",
			Box::pin(async {
				loop {
					gum::info!("Job tick");
					Delay::new(Duration::from_secs(1)).await;
				}
			}),
		)
		.unwrap();

		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					gum::info!("Subsystem2 received message {:?}", msg);
					continue
				},
				Ok(None) => {
					pending!();
				},
				Err(_) => {
					gum::info!("exiting");
					return
				},
			}
		}
	}
}

#[overseer::subsystem(CandidateValidation, error = SubsystemError, prefix = self::overseer)]
impl<Context> Subsystem2 {
	fn start(self, ctx: Context) -> SpawnedSubsystem<SubsystemError> {
		let future = Box::pin(async move {
			Self::run(ctx).await;
			Ok(())
		});

		SpawnedSubsystem { name: "subsystem-2", future }
	}
}

fn main() {
	femme::with_level(femme::LevelFilter::Trace);
	let spawner = sp_core::testing::TaskExecutor::new();
	futures::executor::block_on(async {
		let timer_stream = stream::repeat(()).then(|_| async {
			Delay::new(Duration::from_secs(1)).await;
		});

		let (overseer, _handle) = dummy_overseer_builder(spawner, AlwaysSupportsParachains, None)
			.unwrap()
			.replace_candidate_validation(|_| Subsystem2)
			.replace_candidate_backing(|orig| orig)
			.replace_candidate_backing(|_orig| Subsystem1)
			.build()
			.unwrap();

		let overseer_fut = overseer.run().fuse();
		let timer_stream = timer_stream;

		pin_mut!(timer_stream);
		pin_mut!(overseer_fut);

		loop {
			select! {
				_ = overseer_fut => break,
				_ = timer_stream.next() => {
					gum::info!("tick");
				}
				complete => break,
			}
		}
	});
}
