//! A minimal demo to be used with cargo expand.

use polkadot_overseer_gen::{SpawnNamed, self as overseer, *};
mod misc;

pub use self::misc::*;


#[overlord(signal=SigSigSig, event=EvX, error=Yikes, network=NetworkMsg, gen=AllMessages)]
struct Solo<T> {
	#[subsystem(no_dispatch, consumes: Plinko, sends: [MsgStrukt])]
	goblin_tower: GoblinTower,
}


#[derive(Default)]
pub struct GoblinTower;

impl<Ctx> overseer::Subsystem<Ctx, Yikes> for GoblinTower
where
	Ctx: GoblinTowerContextTrait,
	<Ctx as GoblinTowerContextTrait>::Sender: GoblinTowerSenderTrait,
	<Ctx as overseer::SubsystemContext>::Sender: GoblinTowerSenderTrait,
{
	fn start(self, mut ctx: Ctx) -> SpawnedSubsystem<Yikes> {
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
	let (overseer, _handle): (Solo<_, f64>, _) = Solo::builder()
		.goblin_tower(GoblinTower::default())
		.spawner(DummySpawner)
		.build()
		.unwrap();
}
