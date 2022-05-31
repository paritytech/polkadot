// Copyright (C) 2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(dead_code)] // orchestra events are not used

//! A minimal demo to be used with cargo expand.

use orchestra::{self as orchestra, Spawner, *};
mod misc;

pub use self::misc::*;

#[orchestra(signal=SigSigSig, event=EvX, error=Yikes, gen=AllMessages)]
struct Solo<T> {
	#[subsystem(consumes: Plinko, sends: [MsgStrukt])]
	goblin_tower: GoblinTower,
}

#[derive(Default)]
pub struct Fortified;

#[orchestra::subsystem(GoblinTower, error=Yikes)]
impl<Context> Fortified {
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<Yikes> {
		let mut sender = ctx.sender().clone();
		ctx.spawn(
			"GoblinTower",
			Box::pin(async move {
				sender.send_message(MsgStrukt(8u8)).await;
			}),
		)
		.unwrap();
		unimplemented!("welcum")
	}
}

fn main() {
	use futures::{executor, pin_mut};

	executor::block_on(async move {
		let (orchestra, _handle): (Solo<_>, _) = Solo::builder()
			.goblin_tower(Fortified::default())
			.spawner(DummySpawner)
			.build()
			.unwrap();

		let orchestra_fut = orchestra
			.running_subsystems
			.into_future()
			.timeout(std::time::Duration::from_millis(300))
			.fuse();

		pin_mut!(orchestra_fut);

		orchestra_fut.await
	});
}
