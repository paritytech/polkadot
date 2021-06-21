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

use std::time::Duration;
use futures::{
	channel::oneshot,
	pending, pin_mut, select, stream,
	FutureExt, StreamExt,
};
use futures_timer::Delay;

use polkadot_node_primitives::{PoV, BlockData};
use polkadot_primitives::v1::Hash;
use polkadot_overseer::{Overseer, HeadSupportsParachains, AllSubsystems};

use polkadot_subsystem::{Subsystem, SubsystemContext, SpawnedSubsystem, FromOverseer};
use polkadot_subsystem::messages::{
	CandidateValidationMessage, CandidateBackingMessage, AllMessages,
};

struct AlwaysSupportsParachains;
impl HeadSupportsParachains for AlwaysSupportsParachains {
	fn head_supports_parachains(&self, _head: &Hash) -> bool { true }
}

struct Subsystem1;

impl Subsystem1 {
	async fn run(mut ctx: impl SubsystemContext<Message=CandidateBackingMessage>)  {
		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					if let FromOverseer::Communication { msg } = msg {
						tracing::info!("msg {:?}", msg);
					}
					continue;
				}
				Ok(None) => (),
				Err(_) => {
					tracing::info!("exiting");
					return;
				}
			}

			Delay::new(Duration::from_secs(1)).await;
			let (tx, _) = oneshot::channel();

			ctx.send_message(AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					Default::default(),
					PoV {
						block_data: BlockData(Vec::new()),
					}.into(),
					tx,
				)
			)).await;
		}
	}
}

impl<C> Subsystem<C> for Subsystem1
	where C: SubsystemContext<Message=CandidateBackingMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			Self::run(ctx).await;
			Ok(())
		});

		SpawnedSubsystem {
			name: "subsystem-1",
			future,
		}
	}
}

struct Subsystem2;

impl Subsystem2 {
	async fn run(mut ctx: impl SubsystemContext<Message=CandidateValidationMessage>)  {
		ctx.spawn(
			"subsystem-2-job",
			Box::pin(async {
				loop {
					tracing::info!("Job tick");
					Delay::new(Duration::from_secs(1)).await;
				}
			}),
		).unwrap();

		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					tracing::info!("Subsystem2 received message {:?}", msg);
					continue;
				}
				Ok(None) => { pending!(); }
				Err(_) => {
					tracing::info!("exiting");
					return;
				},
			}
		}
	}
}

impl<C> Subsystem<C> for Subsystem2
	where C: SubsystemContext<Message=CandidateValidationMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			Self::run(ctx).await;
			Ok(())
		});

		SpawnedSubsystem {
			name: "subsystem-2",
			future,
		}
	}
}

fn main() {
	femme::with_level(femme::LevelFilter::Trace);
	let spawner = sp_core::testing::TaskExecutor::new();
	futures::executor::block_on(async {
		let timer_stream = stream::repeat(()).then(|_| async {
			Delay::new(Duration::from_secs(1)).await;
		});

		let all_subsystems = AllSubsystems::<()>::dummy()
			.replace_candidate_validation(Subsystem2)
			.replace_candidate_backing(Subsystem1);
		let (overseer, _handler) = Overseer::new(
			vec![],
			all_subsystems,
			None,
			AlwaysSupportsParachains,
			spawner,
		).unwrap();
		let overseer_fut = overseer.run().fuse();
		let timer_stream = timer_stream;

		pin_mut!(timer_stream);
		pin_mut!(overseer_fut);

		loop {
			select! {
				_ = overseer_fut => break,
				_ = timer_stream.next() => {
					tracing::info!("tick");
				}
				complete => break,
			}
		}
	});
}
