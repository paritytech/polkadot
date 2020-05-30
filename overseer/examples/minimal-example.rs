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
use futures::{pending, pin_mut, executor, select, stream, FutureExt, StreamExt};
use futures_timer::Delay;
use kv_log_macro as log;

use overseer::{Overseer, Subsystem, SubsystemContext, SpawnedSubsystem};

#[derive(Clone, Copy, Debug, Eq, PartialEq, std::hash::Hash)]
pub enum SubsystemId {
	Subsystem1,
	Subsystem2,
	Subsystem3,
}

struct Subsystem1;

impl Subsystem1 {
	async fn run(mut ctx: SubsystemContext<usize, SubsystemId>)  {
		loop {
			match ctx.try_recv().await {
				Ok(Some(msg)) => {
					log::info!("Subsystem1 received message {:?}", msg);
				}
				Ok(None) => (),
				Err(_) => {}
			}

			Delay::new(Duration::from_secs(1)).await;
			if let Err(_) = ctx.send_msg(SubsystemId::Subsystem2, 10).await {
				break;
			}
		}
	}

	fn new() -> Self {
		Self
	}
}

impl Subsystem<usize, SubsystemId> for Subsystem1 {
	fn start(&mut self, ctx: SubsystemContext<usize, SubsystemId>) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			Self::run(ctx).await;
		}))
	}
}

struct Subsystem2;

impl Subsystem2 {
	async fn run(mut ctx: SubsystemContext<usize, SubsystemId>)  {
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
				Err(_) => {}
			}
		}
	}

	fn new() -> Self {
		Self
	}
}

impl Subsystem<usize, SubsystemId> for Subsystem2 {
	fn start(&mut self, ctx: SubsystemContext<usize, SubsystemId>) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			Self::run(ctx).await;
		}))
	}
}

struct Subsystem3;

impl Subsystem<usize, SubsystemId> for Subsystem3 {
	fn start(&mut self, mut ctx: SubsystemContext<usize, SubsystemId>) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			// TODO: ctx actually has to be used otherwise the channels are dropped
			loop {
				// ignore all incoming msgs
				while let Ok(Some(_)) = ctx.try_recv().await {
				}
				log::info!("Subsystem3 tick");
				Delay::new(Duration::from_secs(1)).await;

				pending!();
			}
		}))
	}

	fn can_recv_msg(&self, _msg: &usize) -> bool { false }
}

fn main() {
	femme::with_level(femme::LevelFilter::Trace);
	let spawner = executor::ThreadPool::new().unwrap();

	futures::executor::block_on(async {
		let subsystems: Vec<(SubsystemId, Box<dyn Subsystem<usize, SubsystemId> + Send>)> = vec![
			(SubsystemId::Subsystem1, Box::new(Subsystem1::new())),
			(SubsystemId::Subsystem2, Box::new(Subsystem2::new())),
		];

		let timer_stream = stream::repeat(()).then(|_| async {
			Delay::new(Duration::from_secs(1)).await;
		});

		let (overseer, mut handler) = Overseer::new(subsystems, spawner);
		let overseer_fut = overseer.run().fuse();
		let timer_stream = timer_stream;

		pin_mut!(timer_stream);
		pin_mut!(overseer_fut);

		loop {
			select! {
				_ = overseer_fut => break,
				_ = timer_stream.next() => {
					handler.send_to_subsystem(SubsystemId::Subsystem1, 42usize).await.unwrap();
				}
				complete => break,
			}
		}
	});
}
