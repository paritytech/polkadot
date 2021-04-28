#![allow(dead_code)]

use polkadot_procmacro_overseer_subsystems_gen::AllSubsystemsGen;

#[derive(Clone, AllSubsystemsGen)]
struct AllSubsystems<A> {
    a: A,
    b: u16,
}

fn main() {
    let all = AllSubsystems::<u8> {
        a: 0u8,
        b: 1u16,
    };
    let _all: AllSubsystems<u32> = all.replace_a::<u32>(777_777u32);
}
