#![allow(dead_code)]

use polkadot_procmacro_overseer_gen::overlord;


struct X;

#[derive(Default, Clone, Copy)]
struct AwesomeSubSys;

#[derive(Clone, AllSubsystemsGen)]
#[overlord(Wrapper)]
struct Overseer {
	#[subsystem(X)]
	sub0: AwesomeSubSys,
}

struct Spawner;

fn main() {
	let overseer = Overseer::<Spawner, SubSystems<X>>::builder()
		.sub0(AwesomeSubSys::default())
		.build(Spawner);

	let overseer = overseer.replace_sub0(TequilaInABar::default());
}
