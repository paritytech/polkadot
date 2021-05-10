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

use proc_macro2::TokenStream;
use syn::{parse2, Error, GenericParam, Result};
use std::collections::HashSet;

mod parse;
mod impl_overseer;
mod impl_replace;
mod impl_channels_out;
mod impl_message_wrapper;
mod inc;

use parse::*;
use impl_overseer::*;
use impl_replace::*;
use impl_channels_out::*;
use impl_message_wrapper::*;

#[proc_macro_attribute]
pub fn overlord(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impl_overseer_gen(attr, item).unwrap_or_else(|err| err.to_compile_error()).into()
}

pub(crate) fn impl_overseer_gen(attr: TokenStream, orig: TokenStream) -> Result<proc_macro2::TokenStream> {
	let args = parse_attr(attr)?;
	let message_wrapper = args.wrapper_enum_name;

	let span = proc_macro2::Span::call_site();
	let ds = parse2::<syn::ItemStruct>(orig.clone())?;
	match ds.fields {
		syn::Fields::Named(named) => {
			let overseer_name = ds.ident.clone();

			// collect the indepedentent subsystem generics
			// which need to be carried along, there are the non-generated ones
			let mut orig_generics = ds.generics;

			// remove default types
			let mut baggage_generic_idents = HashSet::with_capacity(orig_generics.params.len());
			orig_generics.params = orig_generics
				.params
				.into_iter()
				.map(|mut generic| {
					match generic {
						GenericParam::Type(ref mut param) => {
							baggage_generic_idents.insert(param.ident.clone());
							param.eq_token = None;
							param.default = None;
						}
						_ => {}
					}
					generic
				})
				.collect();

			let (subsystem, baggage) = parse_overseer_struct_field(baggage_generic_idents, named)?;
			let info = OverseerInfo {
				subsystem,
				baggage,
				overseer_name,
				message_wrapper,
			};

			let mut additive = impl_overseer_struct(orig_generics, &info)?;

			additive.extend(impl_message_wrapper_enum(&info)?);
			additive.extend(impl_channels_out_struct(&info)?);
			additive.extend(impl_replacable_subsystem(&info));

			additive.extend(inc::include_static_rs()?);

			Ok(additive)
		}
		syn::Fields::Unit => Err(Error::new(span, "Must be a struct with named fields. Not an unit struct.")),
		syn::Fields::Unnamed(_) => {
			Err(Error::new(span, "Must be a struct with named fields. Not an unnamed fields struct."))
		}
	}
}



#[cfg(test)]
mod tests {
	use super::*;
	use quote::quote;

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
