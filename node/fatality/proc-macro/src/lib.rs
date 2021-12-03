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

use std::collections::{HashMap, HashSet};

use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{
	parse2,
	spanned::Spanned,
	token::{Brace, Bracket, Paren},
	Item, ItemEnum, Pat, PatLit, PatPath, PatRest, PatStruct, PatTupleStruct, Path, Token, Variant, Attribute,
};

// TODO
// pub mod keywords {
// 	syn::custom_keyword!(fatal);
// }

use proc_macro_crate::{crate_name, FoundCrate};

fn abs_helper_path(what: impl Into<Path>, loco: Span) -> Path {
	let what = what.into();
	let found_crate = if cfg!(test) {
		FoundCrate::Itself
	} else {
		crate_name("fatality").expect("`fatality` is present in `Cargo.toml`")
	};
	let ts = match found_crate {
		FoundCrate::Itself => quote!( crate::#what ),
		FoundCrate::Name(name) => {
			let ident = Ident::new(&name, loco);
			quote!( #ident::#what )
		},
	};
	let path: Path = parse2(ts).unwrap();
	path
}

fn trait_fatality_impl(who: &Ident, logic: TokenStream) -> TokenStream {
	let fatality_trait = abs_helper_path(Ident::new("Fatality", who.span()), who.span());
	quote! {
		impl #fatality_trait for #who {
			fn is_fatal(&self) -> bool {
				#logic
			}
		}
	}
}

use syn::{
	punctuated::Punctuated, token::Colon2, FieldPat, Fields, PatTuple, PathArguments, PathSegment,
};

#[derive(Clone, Debug)]
enum Determine {
	Fatal,
	Jfyi,
	Defer(Ident),
}

impl From<bool> for Determine {
	fn from(b: bool) -> Self {
		if b {
			Self::Fatal
		} else {
			Self::Jfyi
		}
	}
}

impl From<Ident> for Determine {
	fn from(who: Ident) -> Self {
		Self::Defer(who)
	}
}

impl ToTokens for Determine {
	fn to_tokens(&self, ts: &mut TokenStream) {
		let tmp = match self {
			Self::Fatal => quote! { true },
			Self::Jfyi => quote! { false },
			Self::Defer(inner) => quote! { #inner .is_fatal() },
		};
		ts.extend(tmp)
	}
}

/// Returns the pattern to match, and if there is an inner ident
/// that was annotated with source, which would be used to defer
/// `is_fatal` resolution.
fn variant_to_pattern(variant: &Variant, has_fatal_annotation: bool) -> (Pat, Determine) {
	let span = variant.fields.span();

	let me = PathSegment { ident: Ident::new("Self", span), arguments: PathArguments::None };
	let path = Path {
		leading_colon: None,
		segments: Punctuated::<PathSegment, Colon2>::from_iter(vec![
			me,
			variant.ident.clone().into(),
		]),
	};
	let is_transparent = variant
		.attrs
		.iter()
		.find_map(|attr| {
			if attr.path.is_ident(&Ident::new("error", span)) {
				// FIXME check attr is transparent
				Some(true)
			} else {
				None
			}
		})
		.unwrap_or_default();

	// let source: Option<Ident> = variant
	let (pat, resolution) = match variant.fields {
		Fields::Named(ref _named_fields) => (
			Pat::Struct(PatStruct {
				attrs: vec![],
				path,
				brace_token: Brace(span),
				fields: Punctuated::<FieldPat, Token![,]>::new(),
				dot2_token: Some(Token![..](span)),
			}),
			Determine::from(has_fatal_annotation),
		),
		Fields::Unnamed(ref _unnamed_fields) => (
			Pat::TupleStruct(PatTupleStruct {
				attrs: vec![],
				path,
				pat: PatTuple {
					attrs: vec![],
					paren_token: Paren(span),
					elems: Punctuated::<Pat, Token![,]>::from_iter([Pat::Rest(PatRest {
						attrs: vec![],
						dot2_token: Token![..](span),
					})]),
				},
			}),
			// TODO take into account `#[source]` or `#[error(transparent)]` annotations
			Determine::from(has_fatal_annotation),
		),
		Fields::Unit => (
			Pat::Path(PatPath { attrs: vec![], qself: None, path }),
			Determine::from(has_fatal_annotation),
		),
	};
	(pat, resolution)
}

fn trait_conversion_impl(original: &ItemEnum, jfyi: &ItemEnum, fatal: &ItemEnum) -> TokenStream {
	// original.variants
	// jfyi.variants
	// fatal.variants
	TokenStream::new()
}

fn fatality_gen(item: &ItemEnum) -> TokenStream {
	let name = item.ident.clone();
	let mut original = item.clone();

	let mut fatal = original.clone();
	fatal.variants.clear();
	fatal.ident = Ident::new(format!("Fatal{}", name).as_str(), original.span());

	let mut jfyi = original.clone();
	jfyi.variants.clear();
	jfyi.ident = Ident::new(format!("Jfyi{}", name).as_str(), original.span());

	let mut has_fatal_annotation = HashSet::new();

	// if there is not a single fatal annotation, we can just replace `#[fatality]` with `#[derive(thiserror::Error, Debug)]`
	// without the intermediate type. But impl `trait Fatality` on-top.
	for variant in original.variants.iter_mut() {
		let mut is_fatal = false;

		// remove the `#[fatal]` attribute
		while let Some(idx) = variant.attrs.iter().enumerate().find_map(|(idx, attr)| {
			if attr.path.is_ident(&Ident::new("fatal", Span::call_site())) {
				Some(idx)
			} else {
				None
			}
		}) {
			dbg!(&mut variant.attrs).remove(idx);
			dbg!(&mut variant.attrs);
			is_fatal = true;
		}
		// add to the `Fatal*` or `Jfyi*` `enum`
		if is_fatal {
			has_fatal_annotation.insert(variant.clone());
			fatal.variants.push(variant.clone());
		} else {
			jfyi.variants.push(variant.clone());
		}
	}

	let fatal_count = has_fatal_annotation.len();
	let fatal_only = fatal_count == original.variants.len();
	let jfyi_only = fatal_count == 0;

	// we can avoid the entire generation of extra enums if none, or all variants are `fatal` or `jfyi`
	if !fatal_only && !jfyi_only {
		let thiserror: Path = parse2(quote!(thiserror::Error)).unwrap();
		let thiserror = abs_helper_path(thiserror, name.span());
		let vis = original.vis;
		let attrs = original.attrs;
		let name_fatal = &fatal.ident;
		let name_jfyi = &jfyi.ident;
		let wrapper_enum = quote! {
			#[derive(#thiserror,Debug)]
			#vis enum #name {
					#[error(transparent)]
					Fatal(#name_fatal),
					#[error(transparent)]
					Jfyi(#name_jfyi),
			}
		};

		let fatal_enum = quote! {
			#[derive(#thiserror, Debug)]
			#fatal
		};

		let jfyi_enum = quote! {
			#[derive(#thiserror, Debug)]
			#jfyi
		};

		let pattern_and_resolution = original
			.variants
			.iter()
			.map(move |variant| {
				variant_to_pattern(variant, has_fatal_annotation.contains(&*variant))
			})
			.collect::<HashMap<_, _>>();
		let pat = pattern_and_resolution.keys().cloned();
		let resolution = pattern_and_resolution.values().cloned();
		let mut ts = TokenStream::new();
		ts.extend(wrapper_enum);
		ts.extend(trait_fatality_impl(
			&original.ident,
			quote! {
				match self {
					#( #pat => #resolution ),*
				}
			},
		));

		ts.extend(fatal_enum);
		ts.extend(trait_fatality_impl(
			&fatal.ident,
			quote! {
				true
			},
		));

		ts.extend(jfyi_enum);
		ts.extend(trait_fatality_impl(
			&jfyi.ident,
			quote! {
				// TODO, if there is a `#[source]` annotation, and `jfyi(fwd)` annotation, fwd to the inner
				false
			},
		));
		ts
	} else {
		let mut ts = item.to_token_stream();
		ts.extend(trait_fatality_impl(
			&original.ident,
			quote! {
				#fatal_only
			},
		));
		ts
	}
}

fn fatality2(
	attr: proc_macro2::TokenStream,
	input: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
	let item: ItemEnum = match syn::parse2(input.clone()) {
		Err(e) => {
			let mut bail = input.into_token_stream();
			bail.extend(e.to_compile_error());
			return bail
		},
		Ok(item) => item,
	};
	if !attr.is_empty() {
		return syn::Error::new_spanned(attr, "fatality does not take any arguments")
			.into_compile_error()
			.into_token_stream()
	}
	let res = fatality_gen(&item);
	res
}

#[proc_macro_attribute]
pub fn fatality(
	attr: proc_macro::TokenStream,
	input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let attr = TokenStream::from(attr);
	let input = TokenStream::from(input);

	let output: TokenStream = fatality2(attr, input);

	proc_macro::TokenStream::from(output)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn basic_full() {
		let input = quote! {
		enum Kaboom {
			#[fatal]
			#[error(transparent)]
			A(X),
			#[error(transparent)]
			B(Y),
		}
				};
		let output = fatality2(TokenStream::new(), input);
		println!(
			r##">>>>>>>>>>>>>>>>>>>
{}
>>>>>>>>>>>>>>>>>>>"##,
			output.to_string()
		);
		assert_eq!(
			output.to_string(),
			quote! {
			#[derive(crate::thiserror::Error, Debug)]
			enum Kaboom {
				#[error(transparent)]
				Fatal(FatalKaboom),
				#[error(transparent)]
				Jfyi(JfyiKaboom),
			}


			impl crate::Fatality for Kaboom {
				fn is_fatal(&self) -> bool {
					match self {
						Self::Fatal(_) => true,
						Self::Jfyi(_) => false,
					}
				}
			}

			#[derive(crate::thiserror::Error, Debug)]
			enum FatalKaboom {
				#[error(transparent)]
				A(X),
			}

			impl crate::Fatality for FatalKaboom {
				fn is_fatal(&self) -> bool {
					true
				}
			}

			#[derive(crate::thiserror::Error, Debug)]
			enum JfyiKaboom {
				#[error(transparent)]
				B(Y),
			}

			impl crate::Fatality for JfyiKaboom {
				fn is_fatal(&self) -> bool {
					false
				}
			}

				}
			.to_string()
		);
	}
}
