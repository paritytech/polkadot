// Copyright 2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use super::*;

use assert_matches::assert_matches;
use quote::quote;

#[test]
fn x() {
	assert_matches!(
		impl_gum2(
			quote! {
				target: "xyz",
				x = Foo::default(),
				z = ?Game::new(),
				"Foo {p} x {q}",
				p,
				q,
			},
			Level::Warn
		),
		Ok(_)
	);
}

mod roundtrip {
	use super::*;

	macro_rules! roundtrip {
		($whatty:ty | $ts:expr) => {
			let input = $ts;
			let typed = ::syn::parse2::<$whatty>(input).expect("Must parse. qed");
			let downgraded = dbg!(typed.to_token_stream());
			let reparsed = ::syn::parse2::<$whatty>(downgraded).expect("Also. qed");
			assert_eq!(
				dbg!(typed.into_token_stream().to_string()),
				reparsed.into_token_stream().to_string(),
			);
		};
	}

	#[test]
	fn basic() {
		roundtrip! {Target | quote! {target: "foo" } };
		roundtrip! {FormatMarker | quote! {?} };
		roundtrip! {FormatMarker | quote! {%} };
		roundtrip! {FormatMarker | quote! {} };
		roundtrip! {Value | quote! {x = y} };
		roundtrip! {Value | quote! {z} };
		roundtrip! {Value | quote! {?q} };
		roundtrip! {Value | quote! {%etcpp} };
		roundtrip! {Value | quote! {f = f} };
		roundtrip! {Value | quote! {ff = ?ff} };
		roundtrip! {Value | quote! {fff = %fff} };
		roundtrip! {Args | quote! {target: "yes", k=?v, candidate_hash, "But why? {a}", a} };
		roundtrip! {Args | quote! {target: "also", candidate_hash = ?c_hash, "But why?"} };
		roundtrip! {Args | quote! {"Nope? {}", candidate_hash} };
	}

	#[test]
	fn e2e() {
		roundtrip! {Args | quote! {target: "yes", k=?v, candidate_hash, "But why? {a}", a} };
		roundtrip! {Args | quote! {target: "also", candidate_hash = ?c_hash, "But why?"} };
		roundtrip! {Args | quote! { "Nope? But yes {}", candidate_hash} };
	}

	#[test]
	fn sample_w_candidate_hash_aliased() {
		dbg!(impl_gum2(
			quote! {
				target: "bar",
				a = a,
				candidate_hash = %Hash::repeat_byte(0xF0),
				b = ?Y::default(),
				c = ?a,
				"xxx"
			},
			Level::Info
		)
		.unwrap()
		.to_string());
	}

	#[test]

	fn sample_w_candidate_hash_aliased_unnecessary() {
		dbg!(impl_gum2(
			quote! {
				"bar",
				a = a,
				candidate_hash = ?candidate_hash,
				b = ?Y::default(),
				c = ?a,
				"xxx",
			},
			Level::Info
		)
		.unwrap()
		.to_string());
	}

	#[test]
	fn xxx2() {
		dbg!(impl_gum2(
			quote! {
				target: "bar",
				a = a,
				candidate_hash = ?candidate_hash,
				b = ?Y::default(),
				c = a,
				"xxx",
			},
			Level::Trace
		)
		.unwrap()
		.to_string());
	}
}
