#![allow(dead_code)]

use polkadot_overseer_gen::overlord;
trait MetaMeta {}

#[derive(Debug)]
struct MsgStrukt(u8);

#[derive(Default, Clone, Copy)]
struct AwesomeSubSys;

#[overlord(Wrapper)]
#[derive(Clone)]
struct Overseer<T: MetaMeta> {
	#[subsystem(MsgStrukt)]
	sub0: AwesomeSubSys,

	something_else: T,
}

struct Spawner;

fn main() {
	let overseer = Overseer::<Spawner, SubSystems<MsgStrukt>>::builder()
		.sub0(AwesomeSubSys::default())
		.build(Spawner);
}
