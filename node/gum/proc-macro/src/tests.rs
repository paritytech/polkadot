// Copyright (C) Parity Technologies (UK) Ltd.
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

#![allow(clippy::dbg_macro)]

use super::*;

use assert_matches::assert_matches;
use quote::quote;

#[test]
fn smoke() {
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
			assert_matches!(
				::syn::parse2::<$whatty>(input),
				Ok(typed) => {
					let downgraded = dbg!(typed.to_token_stream());
					assert_matches!(::syn::parse2::<$whatty>(downgraded),
					Ok(reparsed) => {
						assert_eq!(
							dbg!(typed.into_token_stream().to_string()),
							reparsed.into_token_stream().to_string(),
						)
					});
				}
			);
		}
	}

	#[test]
	fn u_target() {
		roundtrip! {Target | quote! {target: "foo" } };
	}

	#[test]
	fn u_format_marker() {
		roundtrip! {FormatMarker | quote! {?} };
		roundtrip! {FormatMarker | quote! {%} };
		roundtrip! {FormatMarker | quote! {} };
	}

	#[test]
	fn u_value_w_alias() {
		roundtrip! {Value | quote! {x = y} };
		roundtrip! {Value | quote! {f = f} };
		roundtrip! {Value | quote! {ff = ?ff} };
		roundtrip! {Value | quote! {fff = %fff} };
	}

	#[test]
	fn u_value_bare_w_format_marker() {
		roundtrip! {Value | quote! {?q} };
		roundtrip! {Value | quote! {%etcpp} };

		roundtrip! {ValueWithFormatMarker | quote! {?q} };
		roundtrip! {ValueWithFormatMarker | quote! {%etcpp} };
	}

	#[test]
	fn u_value_bare_w_field_access() {
		roundtrip! {ValueWithFormatMarker | quote! {a.b} };
		roundtrip! {ValueWithFormatMarker | quote! {a.b.cdef.ghij} };
		roundtrip! {ValueWithFormatMarker | quote! {?a.b.c} };
	}

	#[test]
	fn u_args() {
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
		assert_matches!(impl_gum2(
			quote! {
				"bar",
				a = a,
				candidate_hash = ?candidate_hash,
				b = ?Y::default(),
				c = ?a,
				"xxx {} {}",
				a,
				a,
			},
			Level::Info
		), Ok(x) => {
			dbg!(x.to_string())
		});
	}

	#[test]
	fn no_fmt_str_args() {
		assert_matches!(impl_gum2(
			quote! {
				target: "bar",
				a = a,
				candidate_hash = ?candidate_hash,
				b = ?Y::default(),
				c = a,
				"xxx",
			},
			Level::Trace
		), Ok(x) => {
			dbg!(x.to_string())
		});
	}

	#[test]
	fn no_fmt_str() {
		assert_matches!(impl_gum2(
			quote! {
				target: "bar",
				a = a,
				candidate_hash = ?candidate_hash,
				b = ?Y::default(),
				c = a,
			},
			Level::Trace
		), Ok(x) => {
			dbg!(x.to_string())
		});
	}

	#[test]
	fn field_member_as_kv() {
		assert_matches!(impl_gum2(
			quote! {
				target: "z",
				?y.x,
			},
			Level::Info
		), Ok(x) => {
			dbg!(x.to_string())
		});
	}

	#[test]
	fn nested_field_member_as_kv() {
		assert_matches!(impl_gum2(
			quote! {
				target: "z",
				?a.b.c.d.e.f.g,
			},
			Level::Info
		), Ok(x) => {
			dbg!(x.to_string())
		});
	}
}
