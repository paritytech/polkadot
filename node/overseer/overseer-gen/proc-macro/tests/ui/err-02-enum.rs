#![allow(dead_code)]

use polkadot_procmacro_overseer_gen::overlord;

struct Msg(u8);

#[derive(Default, Clone, Copy)]
struct AwesomeSub;

#[derive(Clone, AllSubsystemsGen)]
#[overlord(Wrapper)]
enum Overseer {
	#[subsystem(Msg)]
	Sub0(AwesomeSub),
}

struct Spawner;

fn main() {
	let overseer = Overseer::<Spawner, SubSystems<FooSubSys>>::builder()
		.sub0(FooSubSys::default())
		.build(Spawner);

	// try to replace one subsystem with another that can not handle `X`.
	// since it's missing the trait bound.
	let overseer = overseer.replace_sub0(TequilaInABar::default());
}
