//! A dummy to be used with cargo expand

use polkadot_node_network_protocol::WrongVariant;
use polkadot_overseer_gen::*;
use std::collections::HashMap;

/// Concrete subsystem implementation for `MsgStrukt` msg type.
#[derive(Default)]
pub struct AwesomeSubSys;

impl ::polkadot_overseer_gen::Subsystem<AwesomeSubSysContext, Yikes> for AwesomeSubSys {
	fn start(self, ctx: AwesomeSubSysContext) -> SpawnedSubsystem<Yikes> {
		ctx.spawn("awesome", Box::pin(async move { ctx.send_message(Plinko).await }));
		unimplemented!("starting yay!")
	}
}

#[derive(Default)]
pub struct GoblinTower;

impl ::polkadot_overseer_gen::Subsystem<GoblinTowerContext, Yikes> for GoblinTower {
	fn start(self, ctx: GoblinTowerContext) -> SpawnedSubsystem<Yikes> {
		ctx.spawn("awesome", Box::pin(async move { ctx.send_message(MsgStrukt(0u8)).await }));
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

impl From<polkadot_overseer_gen::OverseerError> for Yikes {
	fn from(_: polkadot_overseer_gen::OverseerError) -> Yikes {
		Yikes
	}
}

impl From<polkadot_overseer_gen::mpsc::SendError> for Yikes {
	fn from(_: polkadot_overseer_gen::mpsc::SendError) -> Yikes {
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
pub enum NetworkMsg {
	A,
	B,
	C,
}

impl NetworkMsg {
	fn focus(&self) -> Result<Self, WrongVariant> {
		Ok(match self {
			Self::B => return Err(WrongVariant),
			Self::A | Self::C => self.clone(),
		})
	}
}

#[overlord(signal=SigSigSig, event=EvX, error=Yikes, network=NetworkMsg, gen=AllMessages)]
struct Yyy<T> {
	#[subsystem(consumes: MsgStrukt, sends: Plinko)]
	sub0: AwesomeSubSys,

	#[subsystem(no_dispatch, blocking, consumes: Plinko, sends: MsgStrukt)]
	plinkos: GoblinTower,

	i_like_pi: f64,
	i_like_generic: T,
	i_like_hash: HashMap<f64, f64>,
}

#[derive(Debug, Clone)]
struct DummySpawner;

impl SpawnNamed for DummySpawner {
	fn spawn_blocking(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		unimplemented!("spawn blocking {} {}", task_name, subsystem_name.unwrap_or("default"))
	}

	fn spawn(
		&self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		_future: futures::future::BoxFuture<'static, ()>,
	) {
		unimplemented!("spawn {} {}", task_name, subsystem_name.unwrap_or("default"))
	}
}

#[derive(Debug, Clone)]
struct DummyCtx;

fn main() {
	let (overseer, _handle): (Yyy<_, f64>, _) = Yyy::builder()
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
