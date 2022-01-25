#[fatality(splitable)]
enum Yikes {
	#[error("An apple a day")]
	Orange,

	#[fatal]
	#[error("So dead")]
	Dead,
}

fn foo() -> Result<[u8; 32], Yikes> {
	Err(Yikes::Dead)
}

fn i_call_foo() -> Result<(), FatalYikes> {
	// availble via a convenience trait `Nested` that is implemented
	// for any `Result` whose error type implements `Split`.
	let x: Result<[u8; 32], Jfyi> = foo().into_nested()?;
}

fn i_call_foo_too() -> Result<(), FatalYikes> {
	if let Err(jfyi_and_fatal_ones) = foo() {
		// bail if bad, otherwise just log it
		log::warn!("Jfyi: {:?}", jfyi_and_fatal_ones.split()?);
	}
}
