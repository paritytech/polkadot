#![allow(dead_code)]

use polkadot_overseer_gen::overlord;

#[derive(Default)]
struct AwesomeSubSys;

struct SigSigSig;

struct Event;

#[derive(Clone)]
struct MsgStrukt(u8);

#[overlord(signal=SigSigSig, event=Event, gen=AllMessages)]
enum Overseer {
	#[subsystem(MsgStrukt)]
	Sub0(AwesomeSubSys),
}

#[derive(Debug, Clone)]
struct DummySpawner;

struct DummyCtx;

fn main() {
	let overseer = Overseer::<_,_>::builder()
		.sub0(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
