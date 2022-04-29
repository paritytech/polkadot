//! A minimal demo to be used with cargo expand.

use overseer::SubsystemSender as _;
use polkadot_overseer_gen::{self as overseer, SpawnNamed, *};
mod misc;

pub use self::misc::*;

#[overlord(signal=SigSigSig, event=EvX, error=Yikes, gen=AllMessages)]
struct Solo<T> {
	#[subsystem(consumes: Plinko, sends: [MsgStrukt])]
	goblin_tower: GoblinTower,
}

#[derive(Default)]
pub struct GoblinTower;

#[overseer::subsystem(error=Yikes)]
impl<Context> GoblinTower {
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
	let (overseer, _handle): (Solo<_>, _) = Solo::builder()
		.goblin_tower(GoblinTower::default())
		.spawner(DummySpawner)
		.build()
		.unwrap();
}
