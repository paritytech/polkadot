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

//! A dummy to be used with cargo expand

use orchestra::{self as orchestra, Spawner, *};
use std::{collections::HashMap, sync::Arc};
mod misc;

pub use self::misc::*;

/// Concrete subsystem implementation for `MsgStrukt` msg type.
#[derive(Default)]
pub struct AwesomeSubSys;

#[orchestra::subsystem(Awesome, error=Yikes)]
impl<Context> AwesomeSubSys {
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<Yikes> {
		let mut sender = ctx.sender().clone();
		ctx.spawn(
			"AwesomeSubsys",
			Box::pin(async move {
				sender.send_message(Plinko).await;
			}),
		)
		.unwrap();
		unimplemented!("starting yay!")
	}
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

#[orchestra(signal=SigSigSig, event=EvX, error=Yikes, gen=AllMessages)]
struct Duo<T, U, V, W> {
	#[subsystem(consumes: MsgStrukt, sends: [Plinko])]
	sub0: Awesome,

	#[subsystem(blocking, consumes: Plinko, sends: [MsgStrukt])]
	plinkos: GoblinTower,

	i_like_pi: f64,
	i_like_tuple: (f64, f64),
	i_like_generic: Arc<T>,
	i_like_hash: HashMap<(U, V), Arc<W>>,
}

fn main() {
	use futures::{executor, pin_mut};

	executor::block_on(async move {
		let (orchestra, _handle): (Duo<_, f64, u32, f32, f64>, _) = Duo::builder()
			.sub0(AwesomeSubSys::default())
			.plinkos(Fortified::default())
			.i_like_pi(::std::f64::consts::PI)
			.i_like_tuple((::std::f64::consts::PI, ::std::f64::consts::PI))
			.i_like_generic(Arc::new(42.0))
			.i_like_hash(HashMap::new())
			.spawner(DummySpawner)
			.build()
			.unwrap();

		assert_eq!(orchestra.i_like_pi.floor() as i8, 3);
		assert_eq!(orchestra.i_like_generic.floor() as i8, 42);
		assert_eq!(orchestra.i_like_hash.len() as i8, 0);

		let orchestra_fut = orchestra
			.running_subsystems
			.into_future()
			.timeout(std::time::Duration::from_millis(300))
			.fuse();

		pin_mut!(orchestra_fut);

		orchestra_fut.await
	});
}
