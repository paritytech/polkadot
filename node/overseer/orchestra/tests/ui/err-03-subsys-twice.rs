#![allow(dead_code)]

use orchestra::*;

#[derive(Default)]
struct AwesomeSubSys;

#[derive(Clone, Debug)]
struct SigSigSig;

struct Event;

#[derive(Clone, Debug)]
struct MsgStrukt(u8);

#[derive(Clone, Debug)]
struct MsgStrukt2(f64);

#[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OrchestraError)]
struct Orchestra {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	#[subsystem(MsgStrukt2)]
	sub1: AwesomeSubSys,
}

#[derive(Debug, Clone)]
struct DummySpawner;

struct DummyCtx;

fn main() {
	let orchestra = Orchestra::<_,_>::builder()
		.sub0(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
