#![allow(dead_code)]

use polkadot_overseer_all_subsystems_gen::AllSubsystemsGen;

#[derive(Clone, AllSubsystemsGen)]
enum AllSubsystems<A,B> {
	A(A),
	B(B),
}

fn main() {
	let all = AllSubsystems::<u8,u16>::A(0u8);
}
