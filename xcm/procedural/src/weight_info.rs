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

use inflector::Inflector;
use quote::format_ident;

pub fn derive(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let input: syn::DeriveInput = match syn::parse(item) {
		Ok(input) => input,
		Err(e) => return e.into_compile_error().into(),
	};

	let syn::DeriveInput { generics, data, .. } = input;

	match data {
		syn::Data::Enum(syn::DataEnum { variants, .. }) => {
			let methods =
				variants.into_iter().map(|syn::Variant { ident, fields, attrs, .. }| {
					let snake_cased_ident = format_ident!("{}", ident.to_string().to_snake_case());
					let ref_fields = parse_args_attr(attrs).unwrap_or_else(|| {
						fields
							.into_iter()
							.enumerate()
							.map(|(idx, syn::Field { ident, ty, .. })| {
								let field_name = ident.unwrap_or_else(|| format_ident!("_{}", idx));
								let field_ty = match ty {
									syn::Type::Reference(r) => {
										// If the type is already a reference, do nothing
										quote::quote!(#r)
									},
									t => {
										// Otherwise, make it a reference
										quote::quote!(&#t)
									},
								};

								quote::quote!(#field_name: #field_ty,)
							})
							.collect()
					});
					quote::quote!(fn #snake_cased_ident( #ref_fields ) -> Weight;)
				});

			let res = quote::quote! {
				pub trait XcmWeightInfo #generics {
					#(#methods)*
				}
			};
			res.into()
		},
		syn::Data::Struct(syn::DataStruct { struct_token, .. }) => {
			let msg = "structs are not supported by 'derive(XcmWeightInfo)'";
			syn::Error::new(struct_token.span, msg).into_compile_error().into()
		},
		syn::Data::Union(syn::DataUnion { union_token, .. }) => {
			let msg = "unions are not supported by 'derive(XcmWeightInfo)'";
			syn::Error::new(union_token.span, msg).into_compile_error().into()
		},
	}
}

fn parse_args_attr(attrs: Vec<syn::Attribute>) -> Option<proc_macro2::TokenStream> {
	attrs.into_iter().find_map(|attr| {
		attr.path.get_ident().filter(|ident| *ident == "weight_args").and_then(|_| {
			attr.parse_args_with(|stream: syn::parse::ParseStream| {
				let mut fields = Vec::new();
				while !stream.is_empty() {
					let ident: syn::Ident = stream.parse()?;
					let _colon: syn::Token![:] = stream.parse()?;
					let ty: syn::Type = stream.parse()?;
					if stream.peek(syn::Token![,]) {
						let _trailing_comma: syn::Token![,] = stream.parse()?;
					}
					fields.push(quote::quote!(#ident: #ty));
				}
				Ok(quote::quote!( #(#fields),* ))
			})
			.ok()
		})
	})
}
