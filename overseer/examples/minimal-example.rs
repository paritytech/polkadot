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
	pending, pin_mut, executor, select, stream,
	FutureExt, StreamExt,
};
use futures_timer::Delay;
use kv_log_macro as log;

use overseer::{
	AllMessages, CandidateBackingSubsystemMessage, FromOverseer,
	Overseer, Subsystem, SubsystemContext, SpawnedSubsystem, ValidationSubsystemMessage,
};

struct Subsystem1;

impl Subsystem1 {
	async fn run(mut ctx: SubsystemContext<CandidateBackingSubsystemMessage>)  {
		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					if let FromOverseer::Communication { msg } = msg {
						log::info!("msg {:?}", msg);
					}
					continue;
				}
				Ok(None) => (),
				Err(_) => {
					log::info!("exiting");
					return;
				}
			}

			Delay::new(Duration::from_secs(1)).await;
			ctx.send_msg(AllMessages::Validation(
				ValidationSubsystemMessage::ValidityAttestation
			)).await.unwrap();
		}
	}
}

impl Subsystem<CandidateBackingSubsystemMessage> for Subsystem1 {
	fn start(&mut self, ctx: SubsystemContext<CandidateBackingSubsystemMessage>) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			Self::run(ctx).await;
		}))
	}
}

struct Subsystem2;

impl Subsystem2 {
	async fn run(mut ctx: SubsystemContext<ValidationSubsystemMessage>)  {
		ctx.spawn(Box::pin(async {
			loop {
				log::info!("Job tick");
				Delay::new(Duration::from_secs(1)).await;
			}
		})).await.unwrap();

		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					log::info!("Subsystem2 received message {:?}", msg);
					continue;
				}
				Ok(None) => { pending!(); }
				Err(_) => {
					log::info!("exiting");
					return;
				},
			}
		}
	}
}

impl Subsystem<ValidationSubsystemMessage> for Subsystem2 {
	fn start(&mut self, ctx: SubsystemContext<ValidationSubsystemMessage>) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			Self::run(ctx).await;
		}))
	}
}

fn main() {
	femme::with_level(femme::LevelFilter::Trace);
	let spawner = executor::ThreadPool::new().unwrap();

	futures::executor::block_on(async {
		let timer_stream = stream::repeat(()).then(|_| async {
			Delay::new(Duration::from_secs(1)).await;
		});

		let (overseer, _handler) = Overseer::new(
			vec![],
			Box::new(Subsystem2),
			Box::new(Subsystem1),
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
					log::info!("tick");
				}
				complete => break,
			}
		}
	});
}
