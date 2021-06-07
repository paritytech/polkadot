//! A dummy to be used with cargo expand

use std::convert::Infallible;

use polkadot_overseer_gen::*;
use polkadot_subsystem::messages::NetworkBridgeEvent;


/// Concrete subsystem implementation for `MsgStrukt` msg type.
#[derive(Default)]
pub struct AwesomeSubSys;

impl ::polkadot_overseer_gen::Subsystem<XxxSubsystemContext<MsgStrukt>, Yikes> for  AwesomeSubSys {
	fn start(self, ctx: XxxSubsystemContext<MsgStrukt>) -> SpawnedSubsystem < Yikes > {
		unimplemented!("starting yay!")
	}
}

#[derive(Default)]
pub struct GoblinTower;

impl ::polkadot_overseer_gen::Subsystem<XxxSubsystemContext<Plinko>, Yikes> for GoblinTower {
	fn start(self, ctx: XxxSubsystemContext<Plinko>) -> SpawnedSubsystem < Yikes > {
		unimplemented!("welcum")
	}
}


/// A signal sent by the overseer.
#[derive(Debug, Clone)]
pub struct SigSigSig;


/// The external event.
#[derive(Debug, Clone)]
pub struct EvX;


impl EvX {
	pub fn focus<'a, T>(&'a self) -> Result<EvX, ()> {
		unimplemented!("dispatch")
	}
}

#[derive(Debug, Clone, Copy)]
pub struct Yikes;

impl std::fmt::Display for Yikes {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		writeln!(f, "yikes!")
	}
}

impl std::error::Error for Yikes {}

impl From<polkadot_overseer_gen::SubsystemError> for Yikes {
	fn from(_: polkadot_overseer_gen::SubsystemError) -> Yikes {
		Yikes
	}
}

#[derive(Debug, Clone)]
pub struct MsgStrukt(u8);

#[derive(Debug, Clone, Copy)]
pub struct Plinko;

impl From<NetworkMsg> for MsgStrukt {
	fn from(_event: NetworkMsg) -> Self {
		MsgStrukt(1u8)
	}
}


#[derive(Debug, Clone, Copy)]
enum NetworkMsg {
	A,
	B,
	C,
}




#[overlord(signal=SigSigSig, event=EvX, error=Yikes, network=NetworkMsg, gen=AllMessages)]
struct Xxx {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	#[subsystem(no_dispatch, blocking, Plinko)]
	plinkos: GoblinTower,

	i_like_pi: f64,
}

#[derive(Debug, Clone)]
struct DummySpawner;

impl SpawnNamed for DummySpawner{
	fn spawn_blocking(&self, name: &'static str, future: futures::future::BoxFuture<'static, ()>) {
		unimplemented!("spawn blocking")
	}

	fn spawn(&self, name: &'static str, future: futures::future::BoxFuture<'static, ()>) {
		unimplemented!("spawn")
	}
}

#[derive(Debug, Clone)]
struct DummyCtx;

fn main() {
	let overseer = Xxx::builder()
		.sub0(AwesomeSubSys::default())
		.plinkos(GoblinTower::default())
		.i_like_pi(::std::f64::consts::PI)
		.spawner(DummySpawner)
		.build();
}
