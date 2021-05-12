#![allow(dead_code)]

use polkadot_overseer_gen_proc_macro::overlord;

struct X;

#[derive(Default, Clone, Copy)]
struct AwesomeSubSys;

#[derive(Default, Clone, Copy)]
struct SigSigSig;

#[derive(Default, Clone, Copy)]
struct EventX;

#[overlord(signal=self::SigSigSig, event=self::EventX, gen=AllMessages)]
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
