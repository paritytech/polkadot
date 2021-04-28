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

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;

use syn::{Error, GenericParam, Ident, Result, Type, parse2};

#[proc_macro_derive(AllSubsystemsGen)]
pub fn subsystems_gen(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let item: TokenStream = item.into();
	impl_subsystems_gen(item).unwrap_or_else(|err| err.to_compile_error()).into()
}

fn impl_subsystems_gen(item: TokenStream) -> Result<proc_macro2::TokenStream> {
	let span = proc_macro2::Span::call_site();
	let ds = parse2::<syn::ItemStruct>(item.clone())?;

	match ds.fields {
		syn::Fields::Named(named) => {
			#[derive(Clone)]
			struct NameTyTup {
				field: Ident,
				ty: Type,
			}
			let mut orig_generics = ds.generics;
			// remove default types
			orig_generics.params = orig_generics.params.into_iter().map(|mut generic| {
				match generic {
					GenericParam::Type(ref mut param) => {
						param.eq_token = None;
						param.default = None;
					}
					_ => {}
				}
				generic
			}).collect();

			// prepare a hashmap of generic type to member that uses it
			let generic_types = orig_generics.params.iter().filter_map(|generic| {
				if let GenericParam::Type(param) = generic {
					Some(param.ident.clone())
				} else {
					None
				}
			}).collect::<HashSet<Ident>>();

			let strukt_ty = ds.ident;

			if generic_types.is_empty() {
				return Err(Error::new(strukt_ty.span(), "struct must have at least one generic parameter."))
			}

			// collect all fields that exist, and all fields that are replaceable
			let mut replacable_items = Vec::<NameTyTup>::with_capacity(64);
			let mut all_fields = replacable_items.clone();


			let mut duplicate_generic_detection = HashSet::<Ident>::with_capacity(64);

			for field in named.named {
				let field_ident = field.ident.clone().ok_or_else(|| Error::new(span, "Member field must have a name."))?;
				let ty = field.ty.clone();
				let ntt = NameTyTup { field: field_ident, ty };

				replacable_items.push(ntt.clone());


				// assure every generic is used exactly once
				let ty_ident = match field.ty {
					Type::Path(path) => path.path.get_ident().cloned().ok_or_else(|| {
						Error::new(proc_macro2::Span::call_site(),  "Expected an identifier, but got a path.")
					}),
					_ => return Err(Error::new(proc_macro2::Span::call_site(), "Must be path."))
				}?;

				if generic_types.contains(&ty_ident) {
					if let Some(previous) = duplicate_generic_detection.replace(ty_ident) {
						return Err(Error::new(previous.span(), "Generic type parameters may only be used once have at least one generic parameter."))
					}
				}

				all_fields.push(ntt);
			}


			let msg = "Generated by #[derive(AllSubsystemsGen)] derive proc-macro.";
			let mut additive = TokenStream::new();

			// generate an impl of `fn replace_#name`
			for NameTyTup { field: replacable_item, ty: replacable_item_ty } in replacable_items {
				let keeper = all_fields.iter().filter(|ntt| ntt.field != replacable_item).map(|ntt| ntt.field.clone());
				let strukt_ty = strukt_ty.clone();
				let fname = Ident::new(&format!("replace_{}", replacable_item), span);
				// adjust the generics such that the appropriate member type is replaced
				let mut modified_generics = orig_generics.clone();
				modified_generics.params = modified_generics.params.into_iter().map(|mut generic| {
					match generic {
						GenericParam::Type(ref mut param) => {
							param.eq_token = None;
							param.default = None;
							if match &replacable_item_ty {
								Type::Path(path) =>
									path.path.get_ident().filter(|&ident| ident == &param.ident).is_some(),
								_ => false
							} {
								param.ident = Ident::new("NEW", span);
							}
						}
						_ => {}
					}
					generic
				}).collect();

				additive.extend(quote! {
					impl #orig_generics #strukt_ty #orig_generics {
						#[doc = #msg]
						pub fn #fname < NEW > (self, replacement: NEW) -> #strukt_ty #modified_generics {
							#strukt_ty :: #modified_generics {
								#replacable_item: replacement,
								#(
									#keeper: self.#keeper,
								)*
							}
						}
					}
				});
			}

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

	#[test]
	fn basic() {
		let item = quote! {
			pub struct AllSubsystems<A,B,CD> {
				pub a: A,
				pub beee: B,
				pub dj: CD,
			}
		};

		let output = impl_subsystems_gen(item).expect("Simple example always works. qed");
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
