#![allow(dead_code)]

use polkadot_procmacro_overseer_gen::overlord;

struct X;
struct Z;

struct Orange;

#[derive(Default, Clone, Copy)]
struct AwesomeSubSys;


#[derive(Default, Clone, Copy)]
struct TequilaInABar;


#[overlord(Wrapper)]
struct Overseer {
	#[subsystem(X)]
	#[subsystem(Z)]
	sub0: AwesomeSubSys,

	#[subsystem(Orange)]
	shots_of: TequilaInABar,

	other: Stuff,
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
