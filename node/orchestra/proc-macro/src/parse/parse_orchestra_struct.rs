// Copyright (C) 2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use itertools::Itertools;
use proc_macro2::{Span, TokenStream};
use std::collections::{hash_map::RandomState, HashMap, HashSet};
use syn::{
	parenthesized,
	parse::{Parse, ParseStream},
	punctuated::Punctuated,
	spanned::Spanned,
	token::Bracket,
	AttrStyle, Error, Field, FieldsNamed, GenericParam, Ident, ItemStruct, Path, PathSegment,
	Result, Token, Type, Visibility,
};

use quote::{quote, ToTokens};

mod kw {
	syn::custom_keyword!(wip);
	syn::custom_keyword!(blocking);
	syn::custom_keyword!(consumes);
	syn::custom_keyword!(sends);
}

#[derive(Clone, Debug)]
pub(crate) enum SubSysAttrItem {
	/// The subsystem is still a work in progress
	/// and should not be communicated with.
	Wip(kw::wip),
	/// The subsystem is blocking and requires to be
	/// spawned on an exclusive thread.
	Blocking(kw::blocking),
	/// Message to be sent by this subsystem.
	Sends(Sends),
	/// Message to be consumed by this subsystem.
	Consumes(Consumes),
}

impl Parse for SubSysAttrItem {
	fn parse(input: ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();
		Ok(if lookahead.peek(kw::wip) {
			Self::Wip(input.parse::<kw::wip>()?)
		} else if lookahead.peek(kw::blocking) {
			Self::Blocking(input.parse::<kw::blocking>()?)
		} else if lookahead.peek(kw::sends) {
			Self::Sends(input.parse::<Sends>()?)
		} else {
			Self::Consumes(input.parse::<Consumes>()?)
		})
	}
}

impl ToTokens for SubSysAttrItem {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			Self::Wip(wip) => {
				quote! { #wip }
			},
			Self::Blocking(blocking) => {
				quote! { #blocking }
			},
			Self::Sends(_) => {
				quote! {}
			},
			Self::Consumes(_) => {
				quote! {}
			},
		};
		tokens.extend(ts.into_iter());
	}
}

/// A field of the struct annotated with
/// `#[subsystem(A, B, C)]`
#[derive(Clone, Debug)]
pub(crate) struct SubSysField {
	/// Name of the field.
	pub(crate) name: Ident,
	/// Generate generic type name for the `AllSubsystems` type
	/// which is also used `#wrapper_message :: #variant` variant
	/// part.
	pub(crate) generic: Ident,
	/// Type of message to be consumed by the subsystem.
	pub(crate) message_to_consume: Path,
	/// Types of messages to be sent by the subsystem.
	pub(crate) messages_to_send: Vec<Path>,
	/// If the subsystem implementation is blocking execution and hence
	/// has to be spawned on a separate thread or thread pool.
	pub(crate) blocking: bool,
	/// The subsystem is a work in progress.
	/// Avoids dispatching `Wrapper` type messages, but generates the variants.
	/// Does not require the subsystem to be instantiated with the builder pattern.
	pub(crate) wip: bool,
}

// Converts a type enum to a path if this type is a TypePath
fn try_type_to_path(ty: &Type, span: Span) -> Result<Path> {
	match ty {
		Type::Path(path) => Ok(path.path.clone()),
		_ => Err(Error::new(span, "Type must be a path expression.")),
	}
}

// Converts a Rust type to a list of idents recursively checking the possible values
fn flatten_type(ty: &Type, span: Span) -> Result<Vec<Ident>> {
	match ty {
		syn::Type::Array(ar) => flatten_type(&ar.elem, span),
		syn::Type::Paren(par) => flatten_type(&par.elem, span),
		syn::Type::Path(type_path) => type_path
			.path
			.segments
			.iter()
			.map(|seg| flatten_path_segments(seg, span.clone()))
			.flatten_ok()
			.collect::<Result<Vec<_>>>(),
		syn::Type::Tuple(tup) => tup
			.elems
			.iter()
			.map(|element| flatten_type(element, span.clone()))
			.flatten_ok()
			.collect::<Result<Vec<_>>>(),
		_ => Err(Error::new(span, format!("Unsupported type: {:?}", ty))),
	}
}

// Flatten segments of some path to a list of idents used in these segments
fn flatten_path_segments(path_segment: &PathSegment, span: Span) -> Result<Vec<Ident>> {
	let mut result = vec![path_segment.ident.clone()];

	match &path_segment.arguments {
		syn::PathArguments::AngleBracketed(args) => {
			let mut recursive_idents = args
				.args
				.iter()
				.map(|generic_argument| match generic_argument {
					syn::GenericArgument::Type(ty) => flatten_type(ty, span.clone()),
					_ => Err(Error::new(
						span,
						format!(
							"Field has a generic with an unsupported parameter {:?}",
							generic_argument
						),
					)),
				})
				.flatten_ok()
				.collect::<Result<Vec<_>>>()?;
			result.append(&mut recursive_idents);
		},
		syn::PathArguments::None => {},
		_ =>
			return Err(Error::new(
				span,
				format!(
					"Field has a generic with an unsupported path {:?}",
					path_segment.arguments
				),
			)),
	}

	Ok(result)
}

macro_rules! extract_variant {
	($unique:expr, $variant:ident ; default = $fallback:expr) => {
		extract_variant!($unique, $variant).unwrap_or_else(|| $fallback)
	};
	($unique:expr, $variant:ident ; err = $err:expr) => {
		extract_variant!($unique, $variant).ok_or_else(|| Error::new(Span::call_site(), $err))
	};
	($unique:expr, $variant:ident take) => {
		$unique.values().find_map(|item| {
			if let SubSysAttrItem::$variant(value) = item {
				Some(value.clone())
			} else {
				None
			}
		})
	};
	($unique:expr, $variant:ident) => {
		$unique.values().find_map(|item| {
			if let SubSysAttrItem::$variant(_) = item {
				Some(true)
			} else {
				None
			}
		})
	};
}

#[derive(Debug, Clone)]
pub(crate) struct Sends {
	#[allow(dead_code)]
	pub(crate) keyword_sends: kw::sends,
	#[allow(dead_code)]
	pub(crate) colon: Token![:],
	#[allow(dead_code)]
	pub(crate) bracket: Option<Bracket>,
	pub(crate) sends: Punctuated<Path, Token![,]>,
}

impl Parse for Sends {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let content;
		let keyword_sends = input.parse()?;
		let colon = input.parse()?;
		let (bracket, sends) = if !input.peek(syn::token::Bracket) {
			let mut sends = Punctuated::new();
			sends.push_value(input.parse::<Path>()?);
			(None, sends)
		} else {
			let bracket = Some(syn::bracketed!(content in input));
			let sends = Punctuated::parse_terminated(&content)?;
			(bracket, sends)
		};
		Ok(Self { keyword_sends, colon, bracket, sends })
	}
}

#[derive(Debug, Clone)]
pub(crate) struct Consumes {
	#[allow(dead_code)]
	pub(crate) keyword_consumes: Option<kw::consumes>,
	#[allow(dead_code)]
	pub(crate) colon: Option<Token![:]>,
	pub(crate) consumes: Path,
}

impl Parse for Consumes {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();
		Ok(if lookahead.peek(kw::consumes) {
			Self {
				keyword_consumes: Some(input.parse()?),
				colon: input.parse()?,
				consumes: input.parse()?,
			}
		} else {
			Self { keyword_consumes: None, colon: None, consumes: input.parse()? }
		})
	}
}

/// Parses `(Foo, sends = [Bar, Baz])`
/// including the `(` and `)`.
#[derive(Debug, Clone)]
pub(crate) struct SubSystemAttrItems {
	/// The subsystem is in progress, only generate the `Wrapper` variant, but do not forward messages
	/// and also not include the subsystem in the list of subsystems.
	pub(crate) wip: bool,
	/// If there are blocking components in the subsystem and hence it should be
	/// spawned on a dedicated thread pool for such subssytems.
	pub(crate) blocking: bool,
	/// The message type being consumed by the subsystem.
	pub(crate) consumes: Option<Consumes>,
	pub(crate) sends: Option<Sends>,
}

impl Parse for SubSystemAttrItems {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let span = input.span();

		let content;
		let _paren_token = parenthesized!(content in input);

		let items = content.call(Punctuated::<SubSysAttrItem, Token![,]>::parse_terminated)?;

		let mut unique = HashMap::<
			std::mem::Discriminant<SubSysAttrItem>,
			SubSysAttrItem,
			RandomState,
		>::default();

		for item in items {
			if let Some(first) = unique.insert(std::mem::discriminant(&item), item.clone()) {
				let mut e =
					Error::new(item.span(), "Duplicate definition of subsystem attribute found");
				e.combine(Error::new(first.span(), "previously defined here."));
				return Err(e)
			}
		}

		// A subsystem makes no sense if not one of them is provided
		let sends = extract_variant!(unique, Sends take);
		let consumes = extract_variant!(unique, Consumes take);
		if sends.as_ref().map(|sends| sends.sends.is_empty()).unwrap_or(true) && consumes.is_none()
		{
			return Err(Error::new(
				span,
				"Must have at least one of `consumes: [..]` and `sends: [..]`.",
			))
		}

		let blocking = extract_variant!(unique, Blocking; default = false);
		let wip = extract_variant!(unique, Wip; default = false);

		Ok(Self { blocking, wip, sends, consumes })
	}
}

/// Fields that are _not_ subsystems.
#[derive(Debug, Clone)]
pub(crate) struct BaggageField {
	pub(crate) field_name: Ident,
	pub(crate) field_ty: Type,
	pub(crate) generic_types: Vec<Ident>,
	pub(crate) vis: Visibility,
}

#[derive(Clone, Debug)]
pub(crate) struct OrchestraInfo {
	/// Where the support crate `::orchestra` lives.
	pub(crate) support_crate: Path,

	/// Fields annotated with `#[subsystem(..)]`.
	pub(crate) subsystems: Vec<SubSysField>,
	/// Fields that do not define a subsystem,
	/// but are mere baggage.
	pub(crate) baggage: Vec<BaggageField>,
	/// Name of the wrapping enum for all messages, defaults to `AllMessages`.
	pub(crate) message_wrapper: Ident,
	/// Name of the orchestra struct, used as a prefix for
	/// almost all generated types.
	pub(crate) orchestra_name: Ident,

	/// Size of the bounded channel.
	pub(crate) message_channel_capacity: usize,
	/// Size of the bounded signal channel.
	pub(crate) signal_channel_capacity: usize,

	/// Signals to be sent, sparse information that is used intermittently.
	pub(crate) extern_signal_ty: Path,

	/// Incoming event type from the outer world, usually an external framework of some sort.
	pub(crate) extern_event_ty: Path,

	/// Type of messages that are sent to an external subsystem.
	/// Merely here to be included during generation of `#message_wrapper` type.
	pub(crate) outgoing_ty: Option<Path>,

	/// Incoming event type from the outer world, commonly from the network.
	pub(crate) extern_error_ty: Path,
}

impl OrchestraInfo {
	pub(crate) fn support_crate_name(&self) -> &Path {
		&self.support_crate
	}

	pub(crate) fn variant_names(&self) -> Vec<Ident> {
		self.subsystems.iter().map(|ssf| ssf.generic.clone()).collect::<Vec<_>>()
	}

	pub(crate) fn variant_names_without_wip(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|ssf| ssf.generic.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn variant_names_only_wip(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| ssf.wip)
			.map(|ssf| ssf.generic.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn subsystems(&self) -> &[SubSysField] {
		self.subsystems.as_slice()
	}

	pub(crate) fn subsystem_names_without_wip(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|ssf| ssf.name.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn subsystem_generic_types(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|sff| sff.generic.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn baggage(&self) -> &[BaggageField] {
		self.baggage.as_slice()
	}

	pub(crate) fn baggage_names(&self) -> Vec<Ident> {
		self.baggage.iter().map(|bag| bag.field_name.clone()).collect::<Vec<_>>()
	}
	pub(crate) fn baggage_decl(&self) -> Vec<TokenStream> {
		self.baggage
			.iter()
			.map(|bag| {
				let BaggageField { vis, field_ty, field_name, .. } = bag;
				quote! { #vis #field_name: #field_ty }
			})
			.collect::<Vec<TokenStream>>()
	}

	pub(crate) fn baggage_generic_types(&self) -> Vec<Ident> {
		self.baggage
			.iter()
			.flat_map(|bag| bag.generic_types.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn any_message(&self) -> Vec<Path> {
		self.subsystems
			.iter()
			.map(|ssf| ssf.message_to_consume.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn channel_names_without_wip(&self, suffix: &'static str) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|ssf| Ident::new(&(ssf.name.to_string() + suffix), ssf.name.span()))
			.collect::<Vec<_>>()
	}

	pub(crate) fn consumes_without_wip(&self) -> Vec<Path> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|ssf| ssf.message_to_consume.clone())
			.collect::<Vec<_>>()
	}
}

/// Internals of the orchestra.
#[derive(Debug, Clone)]
pub(crate) struct OrchestraGuts {
	pub(crate) name: Ident,
	pub(crate) subsystems: Vec<SubSysField>,
	pub(crate) baggage: Vec<BaggageField>,
}

impl OrchestraGuts {
	pub(crate) fn parse_fields(
		name: Ident,
		baggage_generics: HashSet<Ident>,
		fields: FieldsNamed,
	) -> Result<Self> {
		let n = fields.named.len();
		let mut subsystems = Vec::with_capacity(n);
		let mut baggage = Vec::with_capacity(n);

		// The types of `#[subsystem(..)]` annotated fields
		// have to be unique, since they are used as generics
		// for the builder pattern besides other places.
		let mut unique_subsystem_idents = HashSet::<Ident>::new();
		for Field { attrs, vis, ident, ty, .. } in fields.named.into_iter() {
			// collect all subsystem annotations per field
			let mut subsystem_attr =
				attrs.iter().filter(|attr| attr.style == AttrStyle::Outer).filter_map(|attr| {
					let span = attr.path.span();
					attr.path.get_ident().filter(|ident| *ident == "subsystem").map(move |_ident| {
						let attr_tokens = attr.tokens.clone();
						(attr_tokens, span)
					})
				});
			let ident = ident.ok_or_else(|| {
				Error::new(
					ty.span(),
					"Missing identifier for field, only named fields are expected.",
				)
			})?;

			// a `#[subsystem(..)]` annotation exists
			if let Some((attr_tokens, span)) = subsystem_attr.next() {
				if let Some((_attr_tokens2, span2)) = subsystem_attr.next() {
					return Err({
						let mut err = Error::new(span, "The first subsystem annotation is at");
						err.combine(Error::new(span2, "but another here for the same field."));
						err
					})
				}

				let span = attr_tokens.span();

				let attr_tokens = attr_tokens.clone();
				let subsystem_attrs: SubSystemAttrItems = syn::parse2(attr_tokens.clone())?;

				let field_ty = try_type_to_path(&ty, span)?;
				let generic = field_ty
					.get_ident()
					.ok_or_else(|| {
						Error::new(
							field_ty.span(),
							"Must be an identifier, not a path. It will be used as a generic.",
						)
					})?
					.clone();
				// check for unique subsystem name, otherwise we'd create invalid code:
				if let Some(previous) = unique_subsystem_idents.get(&generic) {
					let mut e = Error::new(generic.span(), "Duplicate subsystem names");
					e.combine(Error::new(previous.span(), "previously defined here."));
					return Err(e)
				}
				unique_subsystem_idents.insert(generic.clone());

				let SubSystemAttrItems { wip, blocking, consumes, sends, .. } = subsystem_attrs;

				// messages to be sent
				let sends = if let Some(sends) = sends {
					Vec::from_iter(sends.sends.iter().cloned())
				} else {
					vec![]
				};
				// messages deemed for consumption
				let consumes = if let Some(consumes) = consumes {
					consumes.consumes
				} else {
					return Err(Error::new(span, "Must provide exactly one consuming message type"))
				};

				subsystems.push(SubSysField {
					name: ident,
					generic,
					message_to_consume: consumes,
					messages_to_send: sends,
					wip,
					blocking,
				});
			} else {
				let flattened = flatten_type(&ty, ident.span())?;
				let generic_types = flattened
					.iter()
					.filter(|flat_ident| baggage_generics.contains(flat_ident))
					.cloned()
					.collect::<Vec<_>>();
				baggage.push(BaggageField { field_name: ident, generic_types, field_ty: ty, vis });
			}
		}
		Ok(Self { name, subsystems, baggage })
	}
}

impl Parse for OrchestraGuts {
	fn parse(input: ParseStream) -> Result<Self> {
		let ds: ItemStruct = input.parse()?;
		match ds.fields {
			syn::Fields::Named(named) => {
				let name = ds.ident.clone();

				// collect the independent subsystem generics
				// which need to be carried along, there are the non-generated ones
				let mut orig_generics = ds.generics;

				// remove defaults from types
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
							},
							_ => {},
						}
						generic
					})
					.collect();

				Self::parse_fields(name, baggage_generic_idents, named)
			},
			syn::Fields::Unit => Err(Error::new(
				ds.fields.span(),
				"Must be a struct with named fields. Not an unit struct.",
			)),
			syn::Fields::Unnamed(unnamed) => Err(Error::new(
				unnamed.span(),
				"Must be a struct with named fields. Not an unnamed fields struct.",
			)),
		}
	}
}
