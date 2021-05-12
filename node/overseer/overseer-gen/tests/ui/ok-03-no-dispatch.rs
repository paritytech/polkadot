#![allow(dead_code)]

use polkadot_overseer_gen::overlord;

#[derive(Debug)]
struct MsgStrukt(u8);

#[derive(Default, Clone, Copy)]
struct AwesomeSubSys;

#[derive(Default, Clone, Copy)]
struct SigSigSig;
#[overlord(signal=SigSigSig, gen=AllMessages)]
struct Overseer<T> {
	#[subsystem(no_dispatch, MsgStrukt)]
	sub0: AwesomeSubSys,

	something_else: T,
}

#[derive(Debug, Clone, Copy)]
struct DummySpawner;

struct Ctx;

fn main() {
	let overseer = Overseer::<_>::builder()
		.sub0(AwesomeSubSys::default())
		.something_else(7777u32)
		.spawner(DummySpawner)
		.build(|| -> Ctx { Ctx } );
}
