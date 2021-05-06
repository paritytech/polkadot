#![allow(dead_code)]

use polkadot_procmacro_overseer_gen::overlord;


#[derive(Debug, Clone)]
struct Foo {
    pub x: String,
}

#[derive(Debug, Clone, Copy)]
struct Bar;

#[derive(Debug, Clone, Copy)]
enum Gap {
    X, Y
}

#[derive(Debug, Clone, Copy)]
struct Arg {
    x: &'static [u8],
}

struct SubsystemA {

}


// generated
struct SubSystems<A,B,C> {}

impl<A,B,C> SubSystemPick for SubSystems<A,B,C>
where
    A: OverseerSubsystem<AllMessages>,
    V: OverseerSubsystem<Message>,
{

}

#[overlord(AllMessages, capacity=1024, signals=64)]
struct Overseer<Metrics, Spawner> {
    #[subsystem(Foo)]
    subsystem_a: SubsystemA,
    // generic A

    #[subsystem(Bar)]
    foo_bar: SubsystemB,
    // generic B

    #[subsystem(Gap)]
    clown: Clown,
    // generic C

    spawner: S,
    metrics: Metrics,
}

impl Overseer<Metrics, Spawner> {
    pub fn subsystems() -> &mut SubSystems<A,B,C> {
        self.subsystems
    }
}

impl Overseer<Metrics, Spawner> {
    fn builder(metrics: Metrics) -> Self {
        OverseerBuilder {

        }
    }
}

const SUBSYSTEM_A: u16 = 0b0001;
const FOO_BAR: u16 = 0b0010;

struct OverseerBuilder<u16> {
    // # all members
    subsystem_a: Option<SubsystemA>,
    foo_bar: ..,
    clown: ..,
}

impl OverseerBuilder<0b0111> {
    // # for all unset members
    fn build<Metrics>(mut self, metrics: Metrics) -> Overseer<Metrics, S> where S: SubSystemPick<A,B,C,..> {
        Overseeer {
            subsystems: SubSystems {
                subsystem_a: self.subsystem_a.unwrap(),
                foo_bar: self.foo_bar.unwrap(),
                clown: self.clown.unwrap(),
            },
        }
    }
}

impl OverseerBuilder<0u16> {
    fn subsystem_a(mut self, subsystem_a: SubsystemA) -> OverseerBuilder<SUBSYTEM_A> {
        self.subsystem_a = Some(subsystem_a);
        self
    }

    fn foo_bar(mut self, foo_bar: FooBar) -> OverseerBuilder<FOO_BAR> {
        // unimplemented!()
    }

    fn clown(mut self) -> OverseerBuilder<CLOWN> {
        self.clown = Some(Clown::default());
    }
}

impl OverseerBuilder<SUBSYTEM_A> {
    fn foo_bar(mut self) -> OverseerBuilder<SUBSYTEM_A| FOO_BAR> {
        // unimplemented!()
    }

    fn clown(mut self) -> OverseerBuilder<SUBSYTEM_A|CLOWN> {
        self.clown = Some(Clown::default());
    }
}


impl OverseerBuilder<SUBSYTEM_A | CLOWN> {
    fn foo_bar(mut self) -> OverseerBuilder<SUBSYTEM_A| CLOWN | FOO_BAR> {
        // unimplemented!()
    }
}

fn main() {
    Overseer::builder()
        .subsystem_a(a, b)
        .foo_bar()
        .build(metrics)

}
