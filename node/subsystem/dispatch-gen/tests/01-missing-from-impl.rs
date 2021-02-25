/// The event type in question.
enum Event {
	Smth,
	Else,
}

/// This should have a `From<Event>` impl but does not.
enum Inner {
	Foo,
	Bar(Event),
}

impl Inner {
	fn focus(&self) -> Option<Self> {
		None
	}
}

#[derive(Clone)]
#[subsystem_dispatch_gen(Event)]
enum AllMessages {
	/// Foo
	Sub1(Inner1),
}
