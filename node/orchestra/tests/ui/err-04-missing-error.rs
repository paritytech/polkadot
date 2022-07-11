#![allow(dead_code)]

use orchestra::*;

#[derive(Default)]
struct AwesomeSubSys;

#[derive(Clone, Debug)]
struct SigSigSig;

struct Event;

#[derive(Clone)]
struct MsgStrukt(u8);

#[orchestra(signal=SigSigSig, event=Event, gen=AllMessages)]
struct Orchestra {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	i_like_pie: f64,
}

#[derive(Debug, Clone)]
struct DummySpawner;

struct DummyCtx;

fn main() {
	let _ = Orchestra::builder()
		.sub0(AwesomeSubSys::default())
		.i_like_pie(std::f64::consts::PI)
		.spawner(DummySpawner)
		.build()
		.unwrap();
}
