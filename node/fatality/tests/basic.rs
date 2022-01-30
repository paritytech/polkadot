use fatality::{fatality, Nested, Split};

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
	let _x: Result<[u8; 32], JfyiYikes> = foo().into_nested()?;
	unreachable!()
}

fn i_call_foo_too() -> Result<(), FatalYikes> {
	if let Err(should_be_fatal) = foo() {
		// bail if bad, otherwise just log it
		log::warn!("Jfyi: {:?}", should_be_fatal.split()?);
	}
	unreachable!()
}

use assert_matches::assert_matches;

#[test]
fn test_i_call_foo_errors() {
	assert_matches!(i_call_foo(), Err(_e));
}

#[test]
fn test_i_call_foo_too_errors() {
	assert_matches!(i_call_foo_too(), Err(_e));
}
