#![allow(dead_code)]

use polkadot_overseer_gen::*;

#[derive(Default)]
struct AwesomeSubSys;

#[derive(Default)]
struct AwesomeSubSys2;

#[derive(Clone, Debug)]
struct SigSigSig;

struct Event;

#[derive(Clone)]
struct MsgStrukt(u8);

#[overlord(signal=SigSigSig, event=Event, gen=AllMessages, error=OverseerError)]
struct Overseer {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	#[subsystem(MsgStrukt)]
	sub1: AwesomeSubSys2,
}

#[derive(Debug, Clone)]
struct DummySpawner;

struct DummyCtx;

fn main() {
	let overseer = Overseer::<_,_>::builder()
		.sub0(AwesomeSubSys::default())
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
