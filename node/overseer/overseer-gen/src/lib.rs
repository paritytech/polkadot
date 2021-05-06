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
use proc_macro2::{Span, TokenStream};
use quote::quote;

use syn::{Attribute, Error, GenericParam, Ident, PathArguments, Result, Type, parse2};
use syn::FieldsNamed;
use syn::AttrStyle;
use syn::Field;
use syn::Variant;
use quote::ToTokens;

#[proc_macro_attribute]
pub fn overlord(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impl_overseer_gen(attr, item).unwrap_or_else(|err| err.to_compile_error()).into()
}

#[derive(Clone)]
struct SubSysField {
	/// Name of the field.
	name: Ident,
	/// Generate generic type name for the `AllSubsystems` type.
	generic: Ident,
	/// Type of the subsystem.
	ty: Ident,
	/// Type to be consumed by the subsystem.
	consumes: Vec<Ident>,
}

fn try_type_to_ident(ty: Type, span: Span) -> Result<Ident> {
	match ty {
		Type::Path(path) => path.path.get_ident().cloned()
		.ok_or_else(|| {
			Error::new(span,  "Expected an identifier, but got a path.")
		}),
		_ => {
			Err(Error::new(span,  "Type must be a path expression."))
		}
	}
}

struct AttrArgs {
	wrapper_enum_name: Ident,
	signal_capacity: usize,
	message_capacity: usize,
}

fn parse_attr(_attr: TokenStream) -> Result<AttrArgs> {
	Ok(AttrArgs {
		wrapper_enum_name: Ident::new("AllMessages", Span::call_site()),
		signal_capacity: 64usize,
		message_capacity: 1024usize,
	})
}

use syn::Token;
use syn::punctuated::Punctuated;
use syn::parse::Parse;
use syn::token::Paren;

struct SubSystemTag {
    attrs: Vec<Attribute>,
	paren_token: Paren,
    consumes: Punctuated<Ident, Token![|]>,
}

impl Parse for SubSystemTag {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let content;
        Ok(Self {
            attrs: Attribute::parse_outer(input)?,
            paren_token: syn::parenthesized!(content in input),
            consumes: content.parse_terminated(Ident::parse)?,
        })
    }
}

/// Creates a list of generic identifiers used for the subsystems
fn parse_overseer_struct_field(baggage_generics: HashSet<Ident>, fields: FieldsNamed) -> Result<(Vec<SubSysField>, Vec<BaggageField>)> {
	let n = fields.named.len();
	let mut subsystems = Vec::with_capacity(n);
	let mut baggage = Vec::with_capacity(n);
	for (idx, Field { attrs, vis, ident, ty, .. }) in fields
		.named
		.into_iter()
		.enumerate() {

		let mut consumes = attrs.iter().filter(|attr| {
			attr.style == AttrStyle::Outer
		}).filter_map(|attr| {
			attr.path.get_ident().filter(|ident| {
				*ident == "subsystem"
			}).map(|_ident| {
				let tokens = attr.tokens.clone();
				tokens
			})
		});

		let mut consumes_idents = Vec::with_capacity(attrs.len());
		for tokens in consumes {
			let variant = syn::parse2::<SubSystemTag>(dbg!(tokens))?;
			consumes_idents.extend(variant.consumes.into_iter());
		}

		let ident = ident.unwrap();
		if !consumes_idents.is_empty() {
			subsystems.push(SubSysField {
				name: ident,
				generic: Ident::new(format!("Sub{}", idx).as_str(), Span::call_site()),
				ty: try_type_to_ident(ty, Span::call_site())?,
				consumes: consumes_idents,
			});
		} else {
			baggage.push(BaggageField {
				field_name: ident,
				field_ty: try_type_to_ident(ty, Span::call_site())?,
				generic: false,// XXX FIXME FIXME FIXME
			});
		}
	}
	Ok((subsystems, baggage))
}

/// Add a feature to replace one subsystem of the subsystems type.
// fn impl_replace_subsystem() -> Result<proc_macro2::TokenStream> {
// 	let orig_generics = vec![];
// 	let added_generics = vec![];
// 	let added_generics_modded = vec![];
// 	let keeper = vec![];
// 	let overseer = Ident::new("Xxxx", Span::call_site());
// 	let msg = "Xyz";
// 	let x = quote!{
// 		impl #orig_generics #overseer #orig_generics {
// 			#[doc = #msg]
// 			pub fn #fname < NEW > (self, replacement: NEW) -> #strukt_ty #modified_generics {
// 				#strukt_ty :: #modified_generics {
// 					#replacable_item: replacement,
// 					#(
// 						#keeper: self.#keeper,
// 					)*
// 				}
// 			}
// 		}
// 	};
// 	Ok(x)
// }

/// Fields that are _not_ subsystems.
struct BaggageField {
	field_name: Ident,
	field_ty: Ident,
	generic: bool,
}

/// Implement a builder pattern.
fn impl_builder(name: Ident, baggage: &[BaggageField], subsystems: &[SubSysField]) -> Result<proc_macro2::TokenStream> {
	let builder = Ident::new((name.to_string() + "Builder").as_str(), Span::call_site());

	let overseer = name.clone();
	let mut orig_generic_ty = &baggage.iter().filter(|b| b.generic).map(|b| b.field_ty.clone()).collect::<Vec<_>>();
	let mut orig_generic_name = &baggage.iter().filter(|b| b.generic).map(|b| b.field_name.clone()).collect::<Vec<_>>();

	let mut field_name = &subsystems.iter().map(|x| x.name.clone() ).collect::<Vec<_>>();
	let mut field_ty = &subsystems.iter().map(|x| x.ty.clone()).collect::<Vec<_>>();

	// FIXME
	let subsystem_generics = Ident::new("FIXME", Span::call_site());

	let x = quote!{

		impl #name {
			fn builder() -> #builder {
				#builder :: default()
			}
		}

		struct #builder {
			#(
				#field_name : ::std::option::Option< #field_ty >,
			)*
		}


		impl #builder {
			#(
				fn #field_name (#field_name: #field_ty ) -> #builder {
					self.#field_name = Some( #field_name );
				}
			)*
		}

		impl #builder {

			fn build< #( #orig_generic_name, )* >(mut self,
				#(
					#orig_generic_name : #orig_generic_ty,
				)*
			) -> #overseer <
					#( #orig_generic_ty , )*,
					SubSystems< #subsystem_generics >
				>  {
				#overseer {
					#(
						#field_name : self. #field_ty .unwrap(),
					)*
				}
			}
		}
	};
	Ok(x)
}

// fn impl_replacable_subsystem(x: Vec<_>) -> Result<proc_macro2::TokenStream> {
// 	let msg = "Generated by #[overlord] derive proc-macro.";
// 	let mut additive = TokenStream::new();

// 	// generate an impl of `fn replace_#name`
// 	for NameTyTup { field: replacable_item, ty: replacable_item_ty } in replacable_items {
// 		let keeper = all_fields.iter().filter(|ntt| ntt.field != replacable_item).map(|ntt| ntt.field.clone());
// 		let strukt_ty = strukt_ty.clone();
// 		let fname = Ident::new(&format!("replace_{}", replacable_item), span);
// 		// adjust the generics such that the appropriate member type is replaced
// 		let mut modified_generics = orig_generics.clone();
// 		modified_generics.params = modified_generics.params.into_iter().map(|mut generic| {
// 			match generic {
// 				GenericParam::Type(ref mut param) => {
// 					param.eq_token = None;
// 					param.default = None;
// 					if match &replacable_item_ty {
// 						Type::Path(path) =>
// 							path.path.get_ident().filter(|&ident| ident == &param.ident).is_some(),
// 						_ => false
// 					} {
// 						param.ident = Ident::new("NEW", span);
// 					}
// 				}
// 				_ => {}
// 			}
// 			generic
// 		}).collect();

// #[derive(Clone)]
// struct NameTyTup {
// 	field: Ident,
// 	ty: Type,
// }

// 		additive.extend(quote! {
// 			impl #orig_generics #strukt_ty #orig_generics {
// 				#[doc = #msg]
// 				pub fn #fname < NEW > (self, replacement: NEW) -> #strukt_ty #modified_generics {
// 					#strukt_ty :: #modified_generics {
// 						#replacable_item: replacement,
// 						#(
// 							#keeper: self.#keeper,
// 						)*
// 					}
// 				}
// 			}
// 		});
// 	}

// 	Ok(additive)
// }


fn impl_messages_wrapper_enum(messages_wrapper: Ident, subsystems: &[SubSysField]) -> Result<proc_macro2::TokenStream> {
	let mut field_ty = subsystems.iter().map(|ssf| {
		ssf.ty.clone()
	});

	let msg = "Generated message type wrapper";
	let x = quote!{
		#[doc = #msg]
		#[derive(Debug, Clone)]
		enum #messages_wrapper {
			#(
				#field_ty ( #field_ty ),
			)*
		}
	};
	Ok(x)
}


fn impl_overseer_struct(overseer_name: Ident, subsystems: &[SubSysField], baggage: &[BaggageField]) -> Result<proc_macro2::TokenStream> {
	let x = quote!{
		struct #overseer_name {
			#(
				#field_ty: field_name,
			)*

			#(
				#baggage_ty: baggage_field_name,
			)*
		}
	};

	x.extend(impl_builder(overseer_name, baggage, subsystems)?);

	Ok(x)
}

fn impl_channels_out_struct(subsystems: &[SubSysField]) -> Result<proc_macro2::TokenStream> {
	let mut field_name = subsystems.iter().map(|ssf| {
		ssf.name.clone()
	});
	let mut field_ty = subsystems.iter().map(|ssf| {
		ssf.ty.clone()
	});
	let x = quote!{
		#[derive(Debug)]
		struct MessagePacket<T> {
			signals_received: usize,
			message: T,
		}

		fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
			MessagePacket {
				signals_received,
				message,
			}
		}

		pub struct ChannelsOut {
			#(
				pub #field_name: ::metered::MeteredSender<MessagePacket< #field_ty >>,
			)*
		}
	};
	Ok(x)
}

fn impl_overseer_gen(attr: TokenStream, orig: TokenStream) -> Result<proc_macro2::TokenStream> {
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
			let mut baggage_generics = HashSet::with_capacity(orig_generics.params.len());
			orig_generics.params = orig_generics.params.into_iter().map(|mut generic| {
				match generic {
					GenericParam::Type(ref mut param) => {
						baggage_generics.insert(param.ident.clone());
						param.eq_token = None;
						param.default = None;
					}
					_ => {}
				}
				generic
			}).collect();

			let (subsystems, bags) = parse_overseer_struct_field(baggage_generics, named)?;

			let mut additive = impl_overseer_struct(&subsystems, &bags);

			additive.extend(impl_messages_wrapper_enum(message_wrapper, &subsystems[..]));
			additive.extend(impl_channels_out_struct(&subsystems[..]));
			// additive.extend(impl_replace_subsystem(&subsystems[..]));
			// additive.extend(impl_(&subsystems[..]));


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
		let attr = quote! {
			overloard(AllMessages, x, y)
		};

		let item = quote! {
			pub struct Ooooh<S> {
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
		println!("{:#?}", output);
	}

	#[test]
	fn ui() {
		let t = trybuild::TestCases::new();
		t.compile_fail("tests/ui/err-*.rs");
		t.pass("tests/ui/ok-*.rs");
	}
}
