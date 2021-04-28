#![allow(dead_code)]

use polkadot_procmacro_overseer_subsystems_gen::AllSubsystemsGen;

#[derive(Clone, AllSubsystemsGen)]
struct AllSubsystems<A=(),B=A> {
	a: A,

    #[sticky]
    b: B,
}

fn main() {
	let all = AllSubsystems::<u8,u16> {
		a: 0u8,
		b: 1u16,
	};
	let all = all.replace_a(77u8);
	// does not exist
	let all = all.replace_b(1999u16);
	let _ = all;
}
