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
use syn::Visibility;
use std::collections::{hash_map::RandomState, HashSet};
use syn::parse::Parse;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::Attribute;
use syn::Field;
use syn::FieldsNamed;
use syn::Ident;
use syn::Token;
use syn::Type;
use syn::{AttrStyle, Path};
use syn::{Error, GenericParam, ItemStruct, Result};

/// A field of the struct annotated with
/// `#[subsystem(no_dispatch, , A | B | C)]`
#[derive(Clone, Debug)]
pub(crate) struct SubSysField {
	/// Name of the field.
	pub(crate) name: Ident,
	/// Generate generic type name for the `AllSubsystems` type.
	pub(crate) generic: Ident,
	/// Type of the subsystem.
	pub(crate) ty: Path,
	/// Type to be consumed by the subsystem.
	pub(crate) consumes: Path,
	/// If `no_dispatch` is present, if the message is incoming via
	/// an extern `Event`, it will not be dispatched to all subsystems.
	pub(crate) no_dispatch: bool,
	/// If the subsystem implementation is blocking execution and hence
	/// has to be spawned on a separate thread or thread pool.
	pub(crate) blocking: bool,
	/// The subsystem is a work in progress.
	/// Avoids dispatching `Wrapper` type messages, but generates the variants.
	/// Does not require the subsystem to be instanciated with the builder pattern.
	pub(crate) wip: bool,
}

fn try_type_to_path(ty: Type, span: Span) -> Result<Path> {
	match ty {
		Type::Path(path) => Ok(path.path),
		_ => Err(Error::new(span, "Type must be a path expression.")),
	}
}

pub(crate) struct SubSystemTag {
	#[allow(dead_code)]
	pub(crate) attrs: Vec<Attribute>,
	#[allow(dead_code)]
	pub(crate) no_dispatch: bool,
	/// The subsystem is WIP, only generate the `Wrapper` variant, but do not forward messages
	/// and also not include the subsystem in the list of subsystems.
	pub(crate) wip: bool,
	pub(crate) blocking: bool,
	pub(crate) consumes: Punctuated<Path, Token![|]>,
}

impl Parse for SubSystemTag {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let attrs = Attribute::parse_outer(input)?;

		let input = input;
		let content;
		let _ = syn::parenthesized!(content in input);
		let parse_tags = || -> Result<Option<Ident>> {
			if content.peek(Ident) && content.peek2(Token![,]) {
				let ident = content.parse::<Ident>()?;
				let _ = content.parse::<Token![,]>()?;
				Ok(Some(ident))
			} else {
				Ok(None)
			}
		};

		let mut unique = HashSet::<_, RandomState>::default();
		while let Some(ident) = parse_tags()? {
			if ident != "no_dispatch" && ident != "blocking" && ident != "wip" {
				return Err(Error::new(ident.span(), "Allowed tags are only `no_dispatch` or `blocking`."));
			}
			if !unique.insert(ident.to_string()) {
				return Err(Error::new(ident.span(), "Found duplicate tag."));
			}
		}
		let no_dispatch = unique.take("no_dispatch").is_some();
		let blocking = unique.take("blocking").is_some();
		let wip = unique.take("wip").is_some();

		let consumes = content.parse_terminated(Path::parse)?;

		Ok(Self { attrs, no_dispatch, blocking, consumes, wip})
	}
}

/// Fields that are _not_ subsystems.
#[derive(Debug, Clone)]
pub(crate) struct BaggageField {
	pub(crate) field_name: Ident,
	pub(crate) field_ty: Path,
	pub(crate) generic: bool,
	pub(crate) vis: Visibility,
}

#[derive(Clone, Debug)]
pub(crate) struct OverseerInfo {
	/// Fields annotated with `#[subsystem(..)]`.
	pub(crate) subsystems: Vec<SubSysField>,
	/// Fields that do not define a subsystem,
	/// but are mere baggage.
	pub(crate) baggage: Vec<BaggageField>,
	/// Name of the wrapping enum for all messages, defaults to `AllMessages`.
	pub(crate) message_wrapper: Ident,
	/// Name of the overseer struct, used as a prefix for
	/// almost all generated types.
	pub(crate) overseer_name: Ident,

	/// Size of the bounded channel.
	pub(crate) message_channel_capacity: usize,
	/// Size of the bounded signal channel.
	pub(crate) signal_channel_capacity: usize,

	/// Signals to be sent, sparse information that is used intermittently.
	pub(crate) extern_signal_ty: Path,

	/// Incoming event type from the outer world, usually an external framework of some sort.
	pub(crate) extern_event_ty: Path,

	/// Incoming event type from an external entity, commonly from the network.
	pub(crate) extern_network_ty: Option<Path>,

	/// Type of messages that are sent to an external subsystem.
	/// Merely here to be included during generation of `message_wrapper` type.
	pub(crate) outgoing_ty: Option<Path>,

	/// Incoming event type from the outer world, commonly from the network.
	pub(crate) extern_error_ty: Path,
}

impl OverseerInfo {
	pub(crate) fn subsystems(&self) -> &[SubSysField] {
		self.subsystems.as_slice()
	}

	pub(crate) fn subsystem_names(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|ssf| ssf.name.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn subsystem_names_without_wip(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|ssf| ssf.name.clone())
			.collect::<Vec<_>>()
	}

	#[allow(dead_code)]
	// TODO use as the defaults, if no subsystem is specified
	// TODO or drop the type argument.
	pub(crate) fn subsystem_types(&self) -> Vec<Path> {
		self.subsystems.iter().map(|ssf| ssf.ty.clone()).collect::<Vec<_>>()
	}

	pub(crate) fn baggage_names(&self) -> Vec<Ident> {
		self.baggage.iter().map(|bag| bag.field_name.clone()).collect::<Vec<_>>()
	}
	pub(crate) fn baggage_types(&self) -> Vec<Path> {
		self.baggage.iter().map(|bag| bag.field_ty.clone()).collect::<Vec<_>>()
	}
	pub(crate) fn baggage_decl(&self) -> Vec<TokenStream> {
		self.baggage
			.iter()
			.map(|bag| {
				let BaggageField {
					vis,
					field_ty,
					field_name,
					..
				} = bag;
				quote!{ #vis #field_name: #field_ty }
			})
			.collect::<Vec<TokenStream>>()
	}

	/// Generic types per subsystem, in the form `Sub#N`.
	pub(crate) fn builder_generic_types(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.filter(|ssf| !ssf.wip)
			.map(|sff| sff.generic.clone())
			.collect::<Vec<_>>()
	}

	pub(crate) fn baggage_generic_types(&self) -> Vec<Ident> {
		self.baggage
			.iter()
			.filter(|bag| bag.generic)
			.filter_map(|bag| bag.field_ty.get_ident().cloned())
			.collect::<Vec<_>>()
	}

	pub(crate) fn channel_names(&self, suffix: &'static str) -> Vec<Ident> {
		self.subsystems
			.iter()
			.map(|ssf| Ident::new(&(ssf.name.to_string() + suffix), ssf.name.span()))
			.collect::<Vec<_>>()
	}

	pub(crate) fn consumes(&self) -> Vec<Path> {
		self.subsystems.iter().map(|ssf| ssf.consumes.clone()).collect::<Vec<_>>()
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
			.map(|ssf| ssf.consumes.clone())
			.collect::<Vec<_>>()
	}
	pub(crate) fn consumes_only_wip(&self) -> Vec<Path> {
		self.subsystems
			.iter()
			.filter(|ssf| ssf.wip)
			.map(|ssf| ssf.consumes.clone())
			.collect::<Vec<_>>()
	}
}

/// Internals of the overseer.
#[derive(Debug, Clone)]
pub(crate) struct OverseerGuts {
	pub(crate) name: Ident,
	pub(crate) subsystems: Vec<SubSysField>,
	pub(crate) baggage: Vec<BaggageField>,
}

impl OverseerGuts {
	pub(crate) fn parse_fields(name: Ident, baggage_generics: HashSet<Ident>, fields: FieldsNamed) -> Result<Self> {
		let n = fields.named.len();
		let mut subsystems = Vec::with_capacity(n);
		let mut baggage = Vec::with_capacity(n);
		for (idx, Field { attrs, vis, ident, ty, .. }) in fields.named.into_iter().enumerate() {
			let mut consumes = attrs.iter().filter(|attr| attr.style == AttrStyle::Outer).filter_map(|attr| {
				let span = attr.path.span();
				attr.path.get_ident().filter(|ident| *ident == "subsystem").map(move |_ident| {
					let attr_tokens = attr.tokens.clone();
					(attr_tokens, span)
				})
			});
			let ident = ident.ok_or_else(|| Error::new(ty.span(), "Missing identifier for member. BUG"))?;

			if let Some((attr_tokens, span)) = consumes.next() {
				if let Some((_attr_tokens2, span2)) = consumes.next() {
					return Err({
						let mut err = Error::new(span, "The first subsystem annotation is at");
						err.combine(Error::new(span2, "but another here for the same field."));
						err
					});
				}
				let mut consumes_paths = Vec::with_capacity(attrs.len());
				let attr_tokens = attr_tokens.clone();
				let variant: SubSystemTag = syn::parse2(attr_tokens.clone())?;
				if variant.consumes.len() != 1 {
					return Err(Error::new(attr_tokens.span(), "Exactly one message can be consumed per subsystem."));
				}
				consumes_paths.extend(variant.consumes.into_iter());

				if consumes_paths.is_empty() {
					return Err(Error::new(span, "Subsystem must consume at least one message"));
				}

				subsystems.push(SubSysField {
					name: ident,
					generic: Ident::new(format!("Sub{}", idx).as_str(), span),
					ty: try_type_to_path(ty, span)?,
					consumes: consumes_paths[0].clone(),
					no_dispatch: variant.no_dispatch,
					wip: variant.wip,
					blocking: variant.blocking,
				});
			} else {
				let field_ty: Path = try_type_to_path(ty, ident.span())?;
				let generic: bool =
					if let Some(ident) = field_ty.get_ident() { baggage_generics.contains(ident) } else { false };
				baggage.push(BaggageField { field_name: ident, generic, field_ty, vis });
			}
		}
		Ok(Self { name, subsystems, baggage })
	}
}

impl Parse for OverseerGuts {
	fn parse(input: ParseStream) -> Result<Self> {
		let ds: ItemStruct = input.parse()?;
		match ds.fields {
			syn::Fields::Named(named) => {
				let name = ds.ident.clone();

				// collect the indepedentent subsystem generics
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
							}
							_ => {}
						}
						generic
					})
					.collect();

				Self::parse_fields(name, baggage_generic_idents, named)
			}
			syn::Fields::Unit => {
				Err(Error::new(ds.fields.span(), "Must be a struct with named fields. Not an unit struct."))
			}
			syn::Fields::Unnamed(unnamed) => {
				Err(Error::new(unnamed.span(), "Must be a struct with named fields. Not an unnamed fields struct."))
			}
		}
	}
}
