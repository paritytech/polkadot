//! A dummy to be used with cargo expand

use polkadot_node_network_protocol::WrongVariant;
use polkadot_overseer_gen::{self as overseer, SpawnNamed, *};
use std::collections::HashMap;
mod misc;

pub use self::misc::*;

/// Concrete subsystem implementation for `MsgStrukt` msg type.
#[derive(Default)]
pub struct AwesomeSubSys;

#[overseer::subsystem(error=Yikes)]
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

#[overlord(signal=SigSigSig, event=EvX, error=Yikes, gen=AllMessages)]
struct Duo<T> {
	#[subsystem(consumes: MsgStrukt, sends: [Plinko])]
	sub0: AwesomeSubSys,

	#[subsystem(blocking, consumes: Plinko, sends: [MsgStrukt])]
	plinkos: GoblinTower,

	i_like_pi: f64,
	i_like_generic: T,
	i_like_hash: HashMap<f64, f64>,
}

fn main() {
	let (overseer, _handle): (Duo<_, f64>, _) = Duo::builder()
		.sub0(AwesomeSubSys::default())
		.plinkos(GoblinTower::default())
		.i_like_pi(::std::f64::consts::PI)
		.i_like_generic(42.0)
		.i_like_hash(HashMap::new())
		.spawner(DummySpawner)
		.build()
		.unwrap();
	assert_eq!(overseer.i_like_pi.floor() as i8, 3);
	assert_eq!(overseer.i_like_generic.floor() as i8, 42);
	assert_eq!(overseer.i_like_hash.len() as i8, 0);
}
