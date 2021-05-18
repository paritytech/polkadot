//! A dummy to be used with cargo expand

use polkadot_overseer_gen::*;

#[derive(Default)]
struct AwesomeSubSys;

#[derive(Debug)]
struct SigSigSig;


#[derive(Debug)]
struct Event;

#[derive(Debug)]
struct Yikes;

impl std::fmt::Display for Yikes {
	fn fmt(&self, mut f: std::fmt::Formatter) -> std::fmt::Result {
		writeln!(f, "yikes!")
	}
}

impl std::error::Error for Yikes {}

impl From<polkadot_overseer_gen::SubsystemError> for Yikes {
	fn from(_: polkadot_overseer_gen::SubsystemError) -> Yikes {
		Yikes
	}
}

#[derive(Clone)]
struct MsgStrukt(u8);

#[overlord(signal=SigSigSig, event=Event, error=Yikes, gen=AllMessages)]
struct Xxx {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,
}

#[derive(Debug, Clone)]
struct DummySpawner;

#[derive(Debug, Clone)]
struct DummyCtx;

fn main() {
	let overseer = Xxx::builder()
		.sub0(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
