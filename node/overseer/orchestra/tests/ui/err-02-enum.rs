#![allow(dead_code)]

use orchestra::*;

#[derive(Default)]
struct AwesomeSubSys;

struct SigSigSig;

struct Event;

#[derive(Clone, Debug)]
struct MsgStrukt(u8);

#[orchestra(signal=SigSigSig, event=Event, gen=AllMessages, error=OrchestraError)]
enum Orchestra {
	#[subsystem(MsgStrukt)]
	Sub0(AwesomeSubSys),
}

#[derive(Debug, Clone)]
struct DummySpawner;

struct DummyCtx;

fn main() {
	let overseer = Orchestra::<_,_>::builder()
		.sub0(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
