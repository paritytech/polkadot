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

use indexmap::IndexMap;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{
	parse::{Parse, ParseStream},
	parse2,
	punctuated::Punctuated,
	spanned::Spanned,
	token::{Brace, Colon2, Paren},
	FieldPat, Fields, ItemEnum, LitBool, Member, Pat, PatIdent, PatPath, PatRest, PatStruct,
	PatTuple, PatTupleStruct, PatWild, Path, PathArguments, PathSegment, Token, Variant,
};

use proc_macro_crate::{crate_name, FoundCrate};

mod kw {
	// Variant fatality is determined based on the inner value, if there is only one, if multiple, the first is chosen.
	syn::custom_keyword!(forward);
	// Scrape the `thiserror` `transparent` annotation.
	syn::custom_keyword!(transparent);
}

#[derive(Clone, Debug)]
enum ResolutionMode {
	None,
	Fatal,
	WithExplicitBool(LitBool),
	Forward(kw::forward, Option<Ident>),
}

impl Default for ResolutionMode {
	fn default() -> Self {
		Self::Fatal
	}
}

impl Parse for ResolutionMode {
	fn parse(input: ParseStream) -> Result<Self, syn::Error> {
		let content;
		let _ = syn::parenthesized!(content in input);

		let lookahead = content.lookahead1();

		if lookahead.peek(kw::forward) {
			Ok(Self::Forward(content.parse::<kw::forward>()?, None))
		} else if lookahead.peek(LitBool) {
			Ok(Self::WithExplicitBool(content.parse::<LitBool>()?))
		} else {
			Err(lookahead.error())
		}
	}
}

impl ToTokens for ResolutionMode {
	fn to_tokens(&self, ts: &mut TokenStream) {
		let tmp = match self {
			Self::None => quote! { false },
			Self::Fatal => quote! { true },
			Self::WithExplicitBool(boolean) => {
				let value = boolean.value;
				quote! { #value }
			},
			Self::Forward(_, maybe_ident) =>
				if let Some(ident) = maybe_ident {
					quote! { #ident .is_fatal() }
				} else {
					quote! { x.is_fatal() }
				},
		};
		ts.extend(tmp)
	}
}

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

#[derive(Debug, Clone)]
struct Transparent(kw::transparent);

impl Parse for Transparent {
	fn parse(input: ParseStream) -> syn::Result<Self> {
		let content;
		let _ = syn::parenthesized!(content in input);

		let lookahead = content.lookahead1();

		if lookahead.peek(kw::transparent) {
			Ok(Self(content.parse::<kw::transparent>()?))
		} else {
			Err(lookahead.error())
		}
	}
}

/// Returns the pattern to match, and if there is an inner ident
/// that was annotated with `#[source]`, which would be used to defer
/// `is_fatal` resolution.
///
/// Consumes a requested `ResolutionMode` and returns the same mode,
/// with a populated identifier, or errors.
fn variant_to_pattern(
	variant: &Variant,
	requested_resolution_mode: ResolutionMode,
) -> Result<(Pat, ResolutionMode), syn::Error> {
	let span = variant.fields.span();
	// default name for referencing a var in an unnamed enum variant
	let pat_capture_ident = &Ident::new("inner", span);
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
		.find(|attr| {
			if attr.path.is_ident(&Ident::new("error", span)) {
				parse2::<Transparent>(attr.tokens.clone()).is_ok()
			} else {
				false
			}
		})
		.is_some();

	let source = Ident::new("source", span);

	// let source: Option<Ident> = variant
	let (pat, resolution) = match variant.fields {
		Fields::Named(ref fields) => {
			let fwd_field = if is_transparent {
				fields.named.first()
			} else {
				fields.named.iter().find(|field| {
					field.attrs.iter().find(|attr| attr.path.is_ident(&source)).is_some()
				})
			}
			.cloned();

			let fields = match requested_resolution_mode {
				ResolutionMode::Forward(_, _) if fwd_field.is_some() => {
					let field_name = fwd_field.as_ref().and_then(|f| f.ident.clone());
					let fp = FieldPat {
						attrs: vec![],
						member: Member::Named(field_name.clone().unwrap()),
						colon_token: None,
						pat: Box::new(Pat::Ident(PatIdent {
							attrs: vec![],
							by_ref: Some(Token![ref](span)),
							mutability: None,
							ident: field_name.unwrap(),
							subpat: None,
						})),
					};
					Punctuated::<FieldPat, Token![,]>::from_iter([fp])
				},
				_ => Punctuated::<FieldPat, Token![,]>::new(),
			};

			(
				Pat::Struct(PatStruct {
					attrs: vec![],
					path,
					brace_token: Brace(span),
					fields,
					dot2_token: Some(Token![..](span)),
				}),
				match requested_resolution_mode {
					ResolutionMode::Forward(fwd, _) => ResolutionMode::Forward(
						fwd,
						fwd_field.as_ref().and_then(|f| f.ident.clone()),
					),
					other => other,
				},
			)
		},
		Fields::Unnamed(ref fields) => {
			let fwd_idx = if is_transparent {
				// take the first one, TODO error if there is more than one
				0_usize
			} else {
				fields
					.unnamed
					.iter()
					.enumerate()
					.find_map(|(idx, field)| {
						field.attrs.iter().find(|attr| attr.path.is_ident(&source)).map(|_attr| idx)
					})
					.ok_or_else(|| {
						syn::Error::new(
							span,
							"Must have a `#[source]` annotated field for `forward` with `fatality`",
						)
					})?
			};

			let maybe_member_ref = if let ResolutionMode::Forward(..) = requested_resolution_mode {
				Some(Pat::Ident(PatIdent {
					attrs: vec![],
					by_ref: Some(Token![ref](span)),
					mutability: None,
					ident: pat_capture_ident.clone(),
					subpat: None,
				}))
			} else {
				None
			};

			let field_pats = std::iter::repeat(Pat::Wild(PatWild {
				attrs: vec![],
				underscore_token: Token![_](span),
			}))
			.take(fwd_idx)
			.chain(maybe_member_ref)
			.chain([Pat::Rest(PatRest { attrs: vec![], dot2_token: Token![..](span) })]);

			(
				Pat::TupleStruct(PatTupleStruct {
					attrs: vec![],
					path,
					pat: PatTuple {
						attrs: vec![],
						paren_token: Paren(span),
						elems: Punctuated::<Pat, Token![,]>::from_iter(field_pats),
					},
				}),
				match requested_resolution_mode {
					ResolutionMode::Forward(x, _) =>
						ResolutionMode::Forward(x, Some(pat_capture_ident.clone())),
					x => x,
				},
			)
		},
		Fields::Unit => (
			Pat::Path(PatPath { attrs: vec![], qself: None, path }),
			match requested_resolution_mode {
				ResolutionMode::Forward(..) =>
					return Err(syn::Error::new(span, "Cannot forward to a unit item variant")),
				resolution => resolution,
			},
		),
	};
	Ok((pat, resolution))
}

fn fatality_gen(item: &ItemEnum) -> Result<TokenStream, syn::Error> {
	let name = item.ident.clone();
	let mut original = item.clone();

	#[cfg(wip)]
	{
	let mut fatal = original.clone();
	fatal.variants.clear();
	fatal.ident = Ident::new(format!("Fatal{}", name).as_str(), original.span());


	let mut jfyi = original.clone();
	jfyi.variants.clear();
	jfyi.ident = Ident::new(format!("Jfyi{}", name).as_str(), original.span());
	}

	let mut has_fatal_annotation = IndexMap::new();

	// if there is not a single fatal annotation, we can just replace `#[fatality]` with `#[derive(thiserror::Error, Debug)]`
	// without the intermediate type. But impl `trait Fatality` on-top.
	for variant in original.variants.iter_mut() {
		let mut resolution_mode = ResolutionMode::None;

		// remove the `#[fatal]` attribute
		while let Some(idx) = variant.attrs.iter().enumerate().find_map(|(idx, attr)| {
			if attr.path.is_ident(&Ident::new("fatal", Span::call_site())) {
				Some(idx)
			} else {
				None
			}
		}) {
			let attr = variant.attrs.remove(idx);
			if attr.tokens.is_empty() {
				resolution_mode = ResolutionMode::Fatal;
			} else {
				resolution_mode = parse2::<ResolutionMode>(attr.tokens.into_token_stream())?;
			}
		}
		// If no `#[fatal]` annotation, this one is non-fatal for sure, statically.
		#[cfg(wip)]
		if let ResolutionMode::None = resolution_mode {
			jfyi.variants.push(variant.clone());
		} else {
			fatal.variants.push(variant.clone());
		}
		has_fatal_annotation.insert(variant.clone(), resolution_mode);
	}

	// Path to `thiserror`.
	let thiserror: Path = parse2(quote!(thiserror::Error)).unwrap();
	let thiserror = abs_helper_path(thiserror, name.span());

	let original_enum = quote! {
		#[derive(#thiserror,Debug)]
		#original
	};

	// Obtain the patterns for each variant, and the resolution, which can either be `forward`, `true`, or `false`
	// as used in the `trait Fatality`.
	let pattern_and_resolution = original
		.variants
		.iter()
		.map(move |variant| {
			variant_to_pattern(
				variant,
				has_fatal_annotation.get(&*variant).cloned().unwrap_or_default(),
			)
		})
		.collect::<Result<IndexMap<_, _>, syn::Error>>()?;
	let pat = pattern_and_resolution.keys().cloned();
	let resolution = pattern_and_resolution.values().cloned();
	let mut ts = TokenStream::new();
	ts.extend(original_enum);
	ts.extend(trait_fatality_impl(
		&original.ident,
		quote! {
			match self {
				#( #pat => #resolution ),*
			}
		},
	));
	Ok(ts)
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
	let res = fatality_gen(&item).unwrap_or_else(|e| e.to_compile_error());
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

	fn run_test(input: TokenStream, expected: TokenStream) {
		let output = fatality2(TokenStream::new(), input);
		let output = output.to_string();
		println!(
			r##">>>>>>>>>>>>>>>>>>>
	{}
	>>>>>>>>>>>>>>>>>>>"##,
			output.as_str()
		);
		assert_eq!(output, expected.to_string(),);
	}

	#[test]
	fn transparent_fatal_implicit() {
		run_test(
			quote! {
				enum Q {
					#[fatal]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(..) => true
						}
					}
				}
			},
		);
	}

	#[test]
	fn transparent_fatal_fwd() {
		run_test(
			quote! {
				enum Q {
					#[fatal(forward)]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]

				enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(ref inner, ..) => inner.is_fatal()
						}
					}
				}
			},
		);
	}

	#[test]
	fn transparent_fatal_true() {
		run_test(
			quote! {
				enum Q {
					#[fatal(true)]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(..) => true
						}
					}
				}
			},
		);
	}

	#[test]
	fn source_fatal() {
		run_test(
			quote! {
				enum Q {
					#[fatal(forward)]
					#[error("DDDDDDDDDDDD")]
					V(first, #[source] I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]

				enum Q {
					#[error("DDDDDDDDDDDD")]
					V(first, #[source] I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(_, ref inner, ..) => inner.is_fatal()
						}
					}
				}
			},
		);
	}

	#[test]
	fn full() {
		run_test(
			quote! {
				enum Kaboom {
					#[fatal(forward)]
					#[error(transparent)]
					A(X),

					#[fatal(forward)]
					#[error("Bar")]
					B(#[source] Y),

					#[fatal(forward)]
					#[error("zzzZZzZ")]
					C {#[source] z: Z },

					#[error("What?")]
					What,


					#[fatal]
					#[error(transparent)]
					O(P),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Kaboom {
					#[error(transparent)]
					A(X),
					#[error("Bar")]
					B(#[source] Y),
					#[error("zzzZZzZ")]
					C {#[source] z: Z },
					#[error("What?")]
					What,
					#[error(transparent)]
					O(P),
				}

				impl crate::Fatality for Kaboom {
					fn is_fatal(&self) -> bool {
						match self {
							Self::A(ref inner, ..) => inner.is_fatal(),
							Self::B(ref inner, ..) => inner.is_fatal(),
							Self::C{ref z, ..} => z.is_fatal(),
							Self::What => false,
							Self::O(..) => true
						}
					}
				}
			},
		);
	}
}
