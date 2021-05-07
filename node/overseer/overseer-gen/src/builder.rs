use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use quote::ToTokens;
use syn::AttrStyle;
use syn::Field;
use syn::FieldsNamed;
use syn::Variant;
use syn::{parse2, Attribute, Error, GenericParam, Ident, PathArguments, Result, Type, TypeParam, WhereClause};

use super::*;

/// Implement a builder pattern.
pub(crate) fn impl_builder(
	name: Ident,
	subsystems: &[SubSysField],
	baggage: &[BaggageField],
) -> Result<proc_macro2::TokenStream> {
	let builder = Ident::new((name.to_string() + "Builder").as_str(), Span::call_site());

	let overseer = name.clone();

	let mut field_name = &subsystems.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
	let mut field_ty = &subsystems.iter().map(|x| x.generic.clone()).collect::<Vec<_>>();

	let mut baggage_generic_ty = &baggage.iter().filter(|b| b.generic).map(|b| b.field_ty.clone()).collect::<Vec<_>>();

	let mut baggage_name = &baggage.iter().map(|x| x.field_name.clone()).collect::<Vec<_>>();
	let mut baggage_ty = &baggage.iter().map(|x| x.field_ty.clone()).collect::<Vec<_>>();

	let generics = quote! {
		< Ctx, #( #baggage_generic_ty, )* #( #field_ty, )* >
	};

	let where_clause = quote! {
		where
			#( #field_ty : Subsystem<Ctx>, )*
	};

	let x = quote! {

		impl #generics #name #generics #where_clause {
			fn builder() -> #builder {
				#builder :: default()
			}
		}

		#[derive(Debug, Clone, Default)]
		struct #builder #generics {
			#(
				#field_name : ::std::option::Option< #field_ty >,
			)*
			#(
				#baggage_name : ::std::option::Option< #baggage_name >,
			)*
		}

		impl #generics #builder #generics #where_clause {
			#(
				fn #field_name (mut self, new: #field_ty ) -> #builder {
					self.#field_name = Some( new );
					self
				}
			)*

			fn build(mut self, ctx: Ctx) -> (#overseer #generics, #handler) {
				let overseer = #overseer :: #generics {
					#(
						#field_name : self. #field_name .unwrap(),
					)*
					#(
						#baggage_name : self. #baggage_name .unwrap(),
					)*
				};
				let handler = #handler {

				};
				(overseer, handler)
			}
		}
	};
	Ok(x)
}
