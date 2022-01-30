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
	// Enum annotation to be splitable.
	syn::custom_keyword!(splitable);
}

#[derive(Clone, Debug)]
enum ResolutionMode {
	/// Not relevant for fatality determination, always non-fatal.
	None,
	/// Fatal by default.
	Fatal,
	/// Specified via a `bool` argument `#[fatal(true)]` or `#[fatal(false)]`.
	WithExplicitBool(LitBool),
	/// Specified via a keyword argument `#[fatal(forward)]`.
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
		crate_name("fatality").expect("`fatality` must be present in `Cargo.toml` for use. q.e.d")
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

fn trait_fatality_impl(
	who: &Ident,
	pattern_and_resolution: &IndexMap<Pat, ResolutionMode>,
) -> TokenStream {
	let pat = pattern_and_resolution.keys();
	let resolution = pattern_and_resolution.values();

	let fatality_trait = abs_helper_path(Ident::new("Fatality", who.span()), who.span());
	quote! {
		impl #fatality_trait for #who {
			fn is_fatal(&self) -> bool {
				match self {
					#( #pat => #resolution, )*
				}
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
			let mut field_pats = if let ResolutionMode::Forward(..) = requested_resolution_mode {
				if is_transparent {
					if fields.unnamed.iter().count() != 1 {
						return Err(
							syn::Error::new(
								fields.span(),
								"Must have exactly one parameter when annotated with `#[transparent]` annotated field for `forward` with `fatality`",
							)
						)
					}
				} else {
					fields
						.unnamed
						.iter()
						.enumerate()
						.find_map(|(idx, field)| {
							field
								.attrs
								.iter()
								.find(|attr| attr.path.is_ident(&source))
								.map(|_attr| idx)
						})
						.ok_or_else(|| {
							syn::Error::new(
								fields.span(),
								"Must have a `#[source]` annotated field for `forward` with `fatality`",
							)
						})?;
				}

				// obtain the i of the i-th unnamed field.
				let fwd_idx = if is_transparent {
					// take the first one, TODO error if there is more than one
					0_usize
				} else {
					fields
						.unnamed
						.iter()
						.enumerate()
						.find_map(|(idx, field)| {
							field
								.attrs
								.iter()
								.find(|attr| attr.path.is_ident(&source))
								.map(|_attr| idx)
						})
						.ok_or_else(|| {
							syn::Error::new(
										span,
										"Must have a `#[source]` annotated field for `forward` with `fatality`",
								)
						})?
				};

				// create a pattern like this: `_, _, _, inner, ..`
				let mut field_pats = std::iter::repeat(Pat::Wild(PatWild {
					attrs: vec![],
					underscore_token: Token![_](span),
				}))
				.take(fwd_idx)
				.collect::<Vec<_>>();

				field_pats.push(Pat::Ident(PatIdent {
					attrs: vec![],
					by_ref: Some(Token![ref](span)),
					mutability: None,
					ident: pat_capture_ident.clone(),
					subpat: None,
				}));

				field_pats
			} else {
				vec![]
			};
			field_pats.push(Pat::Rest(PatRest { attrs: vec![], dot2_token: Token![..](span) }));
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

fn unnamed_fields_variant_pattern_constructor_binding_name(ith: usize) -> Ident {
	Ident::new(format!("arg_{}", ith).as_str(), Span::call_site())
}

#[derive(Hash, Debug)]
struct VariantPattern(Variant);

impl ToTokens for VariantPattern {
	fn to_tokens(&self, ts: &mut TokenStream) {
		let variant_name = &self.0.ident;
		let variant_fields = &self.0.fields;

		match variant_fields {
			Fields::Unit => {
				ts.extend(quote! { #variant_name });
			},
			Fields::Unnamed(unnamed) => {
				let pattern = unnamed
					.unnamed
					.iter()
					.enumerate()
					.map(|(ith, _field)| {
						Pat::Ident(PatIdent {
							attrs: vec![],
							by_ref: None,
							mutability: None,
							ident: unnamed_fields_variant_pattern_constructor_binding_name(ith),
							subpat: None,
						})
					})
					.collect::<Punctuated<Pat, Token![,]>>();
				ts.extend(quote! { #variant_name(#pattern) });
			},
			Fields::Named(named) => {
				let pattern = named
					.named
					.iter()
					.map(|field| {
						Pat::Ident(PatIdent {
							attrs: vec![],
							by_ref: None,
							mutability: None,
							ident: field.ident.clone().expect("Named field has a name. qed"),
							subpat: None,
						})
					})
					.collect::<Punctuated<Pat, Token![,]>>();
				ts.extend(quote! { #variant_name{ #pattern } });
			},
		};
	}
}

/// Constructs an enum variant.
#[derive(Hash, Debug)]
struct VariantConstructor(Variant);

impl ToTokens for VariantConstructor {
	fn to_tokens(&self, ts: &mut TokenStream) {
		let variant_name = &self.0.ident;
		let variant_fields = &self.0.fields;
		ts.extend(match variant_fields {
			Fields::Unit => quote! { #variant_name },
			Fields::Unnamed(unnamed) => {
				let constructor = unnamed
					.unnamed
					.iter()
					.enumerate()
					.map(|(ith, _field)| {
						unnamed_fields_variant_pattern_constructor_binding_name(ith)
					})
					.collect::<Punctuated<Ident, Token![,]>>();
				quote! { #variant_name (#constructor) }
			},
			Fields::Named(named) => {
				let constructor = named
					.named
					.iter()
					.map(|field| field.ident.clone().expect("Named must have named fields. qed"))
					.collect::<Punctuated<Ident, Token![,]>>();
				quote! { #variant_name { #constructor } }
			},
		});
	}
}

// Generate the Jfyi and Fatal sub enums.
fn trait_split_impl(
	attr: Attr,
	original: ItemEnum,
	jfyi_variants: &[Variant],
	fatal_variants: &[Variant],
) -> Result<TokenStream, syn::Error> {
	if let Attr::Empty = attr {
		return Ok(TokenStream::new())
	}

	let span = original.span();

	let thiserror: Path = parse2(quote!(thiserror::Error)).unwrap();
	let thiserror = abs_helper_path(thiserror, span);

	let split_trait = abs_helper_path(Ident::new("Split", span), span);

	let original_ident = original.ident.clone();

	// Generate the splitable types:
	//   Fatal
	let fatal_ident = Ident::new(format!("Fatal{}", original_ident).as_str(), span);
	let mut fatal = original.clone();
	fatal.variants = fatal_variants.iter().cloned().collect();
	fatal.ident = fatal_ident.clone();

	//  Informational (just for your information)
	let jfyi_ident = Ident::new(format!("Jfyi{}", original_ident).as_str(), span);
	let mut jfyi = original.clone();
	jfyi.variants = jfyi_variants.iter().cloned().collect();
	jfyi.ident = jfyi_ident.clone();

	let fatal_patterns = fatal_variants
		.iter()
		.map(|variant| VariantPattern(variant.clone()))
		.collect::<Vec<_>>();
	let jfyi_patterns = jfyi_variants
		.iter()
		.map(|variant| VariantPattern(variant.clone()))
		.collect::<Vec<_>>();

	let fatal_constructors = fatal_variants
		.iter()
		.map(|variant| VariantConstructor(variant.clone()))
		.collect::<Vec<_>>();
	let jfyi_constructors = jfyi_variants
		.iter()
		.map(|variant| VariantConstructor(variant.clone()))
		.collect::<Vec<_>>();

	Ok(quote! {
		impl ::std::convert::From< #fatal_ident> for #original_ident {
			fn from(fatal: #fatal_ident) -> Self {
				match fatal {
					// Fatal
					#( #fatal_ident :: #fatal_patterns => Self:: #fatal_constructors, )*
				}
			}
		}

		impl ::std::convert::From< #jfyi_ident> for #original_ident {
			fn from(jfyi: #jfyi_ident) -> Self {
				match jfyi {
					// JFYI
					#( #jfyi_ident :: #jfyi_patterns => Self:: #jfyi_constructors, )*
				}
			}
		}

		#[derive(#thiserror, Debug)]
		#fatal

		#[derive(#thiserror, Debug)]
		#jfyi

		impl #split_trait for #original_ident {
			type Fatal = #fatal_ident;
			type Jfyi = #jfyi_ident;

			fn split(self) -> ::std::result::Result<Self::Jfyi, Self::Fatal> {
				match self {
					// Fatal
					#( Self :: #fatal_patterns => Err(#fatal_ident :: #fatal_constructors), )*
					// JFYI
					#( Self :: #jfyi_patterns => Ok(#jfyi_ident :: #jfyi_constructors), )*
				}
			}
		}
	})
}

fn fatality_gen(attr: Attr, item: ItemEnum) -> Result<TokenStream, syn::Error> {
	let name = item.ident.clone();
	let mut original = item.clone();

	let mut has_fatal_annotation = IndexMap::new();

	let mut jfyi_variants = Vec::new();
	let mut fatal_variants = Vec::new();

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
		if let ResolutionMode::None = resolution_mode {
			jfyi_variants.push(variant.clone());
		} else {
			fatal_variants.push(variant.clone());
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

	// Obtain the patterns for each variant, and the resolution, which can either
	// be `forward`, `true`, or `false`
	// as used in the `trait Fatality`.
	let pattern_and_resolution = original
		.variants
		.iter()
		.map(move |variant| {
			variant_to_pattern(
				variant,
				has_fatal_annotation.get(&*variant).cloned().unwrap_or_default(),
			)
			.and_then(|x| {
				// currently we don't support, `forward` in combination with `splitable`
				if let Attr::Splitable(keyword_splitable) = attr {
					if let ResolutionMode::Forward(keyword_forward, _) = x.1 {
						let mut err = syn::Error::new_spanned(
							keyword_splitable,
							"Cannot use `split` when using `forward` annotations",
						);
						err.combine(syn::Error::new_spanned(keyword_forward, "used here"));
						return Err(err)
					}
				}
				Ok(x)
			})
		})
		.collect::<Result<IndexMap<_, _>, syn::Error>>()?;

	let mut ts = TokenStream::new();
	ts.extend(original_enum);
	ts.extend(trait_fatality_impl(&original.ident, &pattern_and_resolution));
	ts.extend(trait_split_impl(attr, original, &jfyi_variants, &fatal_variants));
	Ok(ts)
}

/// The declaration of `#[fatality(splitable)]` or `#[fatality]`
/// outside the `enum YourError`.
#[derive(Clone, Copy, Debug)]
enum Attr {
	Splitable(kw::splitable),
	Empty,
}

impl Parse for Attr {
	fn parse(content: ParseStream) -> syn::Result<Self> {
		let lookahead = content.lookahead1();

		if lookahead.peek(kw::splitable) {
			Ok(Self::Splitable(content.parse::<kw::splitable>()?))
		} else if content.is_empty() {
			Ok(Self::Empty)
		} else {
			Err(lookahead.error())
		}
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

	let mut res = TokenStream::new();

	let attr = match syn::parse2::<Attr>(attr) {
		Ok(attr) => attr,
		Err(e) => {
			res.extend(e.to_compile_error());
			Attr::Empty
		},
	};
	res.extend(fatality_gen(attr, item).unwrap_or_else(|e| e.to_compile_error()));

	expander::Expander::new("fatality")
		.add_comment("Generated by `#[fatality::fatality]`".to_owned())
		.dry(true)
		.write_to_out_dir(res)
		.unwrap()
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
mod tests;
