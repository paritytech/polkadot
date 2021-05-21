//! A dummy to be used with cargo expand

use std::convert::Infallible;

use polkadot_overseer_gen::*;
use polkadot_subsystem::messages::NetworkBridgeEvent;


#[derive(Default)]
struct AwesomeSubSys;

#[derive(Debug, Clone)]
struct SigSigSig;


#[derive(Debug, Clone)]
struct EvX;


impl EvX {
	pub fn focus<'a, T>(&'a self) -> Result<EvX, ()> {
		unimplemented!("dispatch")
	}
}

#[derive(Debug, Clone, Copy)]
struct Yikes;

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
struct MsgStrukt(u8);

#[derive(Debug, Clone, Copy)]
struct Plinko;


impl From<EvX> for MsgStrukt {
	fn from(_event: EvX) -> Self {
		MsgStrukt(1u8)
	}
}

#[overlord(signal=SigSigSig, event=EvX, error=Yikes, gen=AllMessages)]
struct Xxx {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	#[subsystem(no_dispatch, Plinko)]
	plinkos: AwesomeSubSys,
}

#[derive(Debug, Clone)]
struct DummySpawner;

#[derive(Debug, Clone)]
struct DummyCtx;

fn main() {
	let overseer = Xxx::builder()
		.sub0(AwesomeSubSys::default())
		.plinkos(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
