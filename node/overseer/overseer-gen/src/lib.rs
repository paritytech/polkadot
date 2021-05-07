// Copyright 2021 Parity Technologies (UK) Ltd.
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

use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use quote::ToTokens;
use syn::AttrStyle;
use syn::Field;
use syn::FieldsNamed;
use syn::Variant;
use syn::{parse2, Attribute, Error, GenericParam, Ident, PathArguments, Result, Type, TypeParam, WhereClause};

mod builder;
mod impls;
mod inc;
mod replace;

use builder::*;
use impls::*;
use inc::*;
use replace::*;

#[proc_macro_attribute]
pub fn overlord(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impls::impl_overseer_gen(attr, item).unwrap_or_else(|err| err.to_compile_error()).into()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn basic() {
		let attr = quote! {
			overloard(AllMessages, x, y)
		};

		let item = quote! {
			pub struct Ooooh<S = Pffffffft> where S: SpawnThat {
				#[subsystem(Foo)]
				sub0: FooSubsystem,

				#[subsystem(Bar | Twain)]
				yyy: BarSubsystem,

				spawner: S,
				metrics: Metrics,
			}
		};

		let output = impl_overseer_gen(attr, item).expect("Simple example always works. qed");
		println!("//generated:");
		println!("{}", output);
	}

	#[test]
	fn ui() {
		let t = trybuild::TestCases::new();
		t.compile_fail("tests/ui/err-*.rs");
		t.pass("tests/ui/ok-*.rs");
	}
}
