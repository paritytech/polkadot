use super::*;
pub use polkadot_primitives::v2::{CandidateHash, Hash};

#[derive(Default, Debug)]
struct Y {
	#[allow(dead_code)]
	x: u8,
}

#[test]
fn wo_alias() {
	let a: i32 = 7;
	error!(target: "foo",
		"Something something {a}, {b:?}, or maybe {c}",
		b = Y::default(),
		c = a
	);
}

#[test]
fn wo_unnecessary() {
	let a: i32 = 7;
	warn!(target: "bar",
		a = a,
		b = ?Y::default(),
		"fff {c}",
		c = a,
	);
}

#[test]
fn w_candidate_hash_value_assignment() {
	let a: i32 = 7;
	info!(target: "bar",
		a = a,
		// ad-hoc value
		candidate_hash = %Hash::repeat_byte(0xF0),
		b = ?Y::default(),
		c = ?a,
		"xxx",
	);
}

#[test]
fn w_candidate_hash_from_scope() {
	let a: i32 = 7;
	let candidate_hash = Hash::repeat_byte(0xF1);
	debug!(target: "bar",
		a = a,
		?candidate_hash,
		b = ?Y::default(),
		c = ?a,
		"xxx",
	);
}

#[test]
fn w_candidate_hash_aliased() {
	let a: i32 = 7;
	let c_hash = Hash::repeat_byte(0xFA);
	trace!(target: "bar",
		a = a,
		candidate_hash = ?c_hash,
		b = ?Y::default(),
		c = a,
		"xxx",
	);
}

#[test]
fn w_candidate_hash_aliased_unnecessary() {
	let a: i32 = 7;
	let candidate_hash = Hash::repeat_byte(0xFA);
	info!(
		target: "bar",
		a = a,
		candidate_hash = ?candidate_hash,
		b = ?Y::default(),
		c = a,
		"xxx",
	);
}
