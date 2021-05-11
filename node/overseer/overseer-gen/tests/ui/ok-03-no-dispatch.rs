#![allow(dead_code)]

use polkadot_overseer_gen_proc_macro::overlord;

#[derive(Debug)]
struct MsgStrukt(u8);

#[derive(Default, Clone, Copy)]
struct AwesomeSubSys;

#[overlord(Wrapper)]
struct Overseer {
	#[subsystem(no_dispatch, MsgStrukt)]
	sub0: AwesomeSubSys,

	something_else: T,
}

#[derive(Debug, Clone, Copy)]
struct DummySpawner;

fn main() {
	let overseer = Overseer::<Ctx, Spawner, MsgStrukt>::builder()
		.sub0(AwesomeSubSys::default())
		.spawner(DummySpawner)
		.build(Ctx);

	let overseer = overseer.replace_sub0(TequilaInABar::default());
}
