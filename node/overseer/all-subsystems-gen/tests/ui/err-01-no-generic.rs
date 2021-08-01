#![allow(dead_code)]

use polkadot_overseer_all_subsystems_gen::AllSubsystemsGen;

#[derive(Clone, AllSubsystemsGen)]
struct AllSubsystems {
	a: f32,
	b: u16,
}

fn main() {
	let all = AllSubsystems {
		a: 0_f32,
		b: 1_u16,
	};
	let _all = all.replace_a(77u8);
}
