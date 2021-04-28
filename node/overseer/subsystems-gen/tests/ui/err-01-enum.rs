#![allow(dead_code)]

use polkadot_procmacro_overseer_subsystems_gen::AllSubsystemsGen;

#[derive(Clone, AllSubsystemsGen)]
enum AllSubsystems<A,B> {
	A(A),

	#[sticky]
	B(B),
}

fn main() {
	let all = AllSubsystems::<u8,u16>::A(0u8);
	// let all = all.replace_a(77u8);
}
