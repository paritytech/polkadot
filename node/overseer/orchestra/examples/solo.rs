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
