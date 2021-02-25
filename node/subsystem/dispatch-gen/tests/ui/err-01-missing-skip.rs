#![allow(dead_code)]

use polkadot_procmacro_subsystem_dispatch_gen::subsystem_dispatch_gen;

/// The event type in question.
#[derive(Clone, Copy)]
enum Event {
	Smth,
	Else,
}

impl Event {
	fn focus(&self) -> std::result::Result<Inner, ()> {
		unimplemented!("foo")
	}
}

/// This should have a `From<Event>` impl but does not.
#[derive(Clone)]
enum Inner {
	Foo,
	Bar(Event),
}

#[subsystem_dispatch_gen(Event)]
#[derive(Clone)]
enum AllMessages {
	/// Foo
	Vvvvvv(Inner),

    /// Missing a `#[skip]` annotation
    Uuuuu,
}

fn main() {
    let _x = AllMessages::dispatch_iter(Event::Else);
}
