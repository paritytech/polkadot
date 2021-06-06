#![allow(dead_code)]

use polkadot_procmacro_overseer_subsystems_gen::AllSubsystemsGen;

#[derive(Clone, AllSubsystemsGen)]
struct AllSubsystems<A, B> {
    a: A,
    b: B,
}

fn main() {
    let all = AllSubsystems::<u8, u16> {
        a: 0u8,
        b: 1u16,
    };
    let _all: AllSubsystems<_,_> = all.replace_a::<u32>(777_777u32);
}
