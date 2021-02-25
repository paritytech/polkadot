#![allow(dead_code)]

use polkadot_procmacro_subsystem_dispatch_gen::subsystem_dispatch_gen;

/// The event type in question.
#[derive(Clone, Copy, Debug)]
enum Event {
	Smth,
	Else,
}

impl Event {
	fn focus(&self) -> std::result::Result<Intermediate, ()> {
		Ok(Intermediate(self.clone()))
	}
}

#[derive(Debug, Clone)]
struct Intermediate(Event);


/// This should have a `From<Event>` impl but does not.
#[derive(Debug, Clone)]
enum Inner {
	Foo,
	Bar(Intermediate),
}

#[subsystem_dispatch_gen(Event)]
#[derive(Clone)]
enum AllMessages {
	/// Foo
	Vvvvvv(Inner),

    #[skip]
    Uuuuu,
}

fn main() {
    let _x = AllMessages::dispatch_iter(Event::Else);
}
