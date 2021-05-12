#![allow(dead_code)]

use polkadot_overseer_gen::overlord;

#[derive(Default)]
struct AwesomeSubSys;

struct SigSigSig;

struct Event;

#[derive(Clone)]
struct MsgStrukt(u8);

#[overlord(signal=SigSigSig, event=Event, gen=AllMessages)]
struct Overseer<T> {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	something_else: T,
}

#[derive(Debug, Clone)]
struct DummySpawner;

struct DummyCtx;


fn main() {
	let overseer = Overseer::<_,_,_>::builder()
		.sub0(AwesomeSubSys::default())
		.something_else(7777u32)
		.spawner(DummySpawner)
		.build(|| -> DummyCtx { DummyCtx } );
}
