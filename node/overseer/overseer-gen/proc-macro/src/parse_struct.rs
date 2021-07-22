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
use std::collections::{hash_map::RandomState, HashSet, HashMap};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::parse::{Parse, ParseStream};
use syn::{
	Attribute, Field, FieldsNamed, Ident, Token, Type, AttrStyle, Path,
	Error, GenericParam, ItemStruct, Result, Visibility
};

use quote::{quote, ToTokens};

mod kw {
	syn::custom_keyword!(wip);
	syn::custom_keyword!(no_dispatch);
	syn::custom_keyword!(blocking);
}


#[derive(Clone, Debug)]
enum SubSysAttrItem {
	/// The subsystem is still a work in progress
	/// and should not be communicated with.
	Wip(kw::wip),
	/// The subsystem is blocking and requires to be
	/// spawned on an exclusive thread.
	Blocking(kw::blocking),
	/// External messages should not be - after being converted -
	/// be dispatched to the annotated subsystem.
	NoDispatch(kw::no_dispatch),
}

impl Parse for SubSysAttrItem {
	fn parse(input: ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();
		Ok(if lookahead.peek(kw::wip) {
			Self::Wip(input.parse::<kw::wip>()?)
		} else if lookahead.peek(kw::blocking) {
			Self::Blocking(input.parse::<kw::blocking>()?)
		} else if lookahead.peek(kw::no_dispatch) {
			Self::NoDispatch(input.parse::<kw::no_dispatch>()?)
		} else {
			return Err(lookahead.error())
		})
	}
}

impl ToTokens for SubSysAttrItem {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			Self::Wip(wip) => { quote!{ #wip } }
			Self::Blocking(blocking) => { quote!{ #blocking } }
			Self::NoDispatch(no_dispatch) => { quote!{ #no_dispatch } }
		};
		tokens.extend(ts.into_iter());
	}
}


/// A field of the struct annotated with
/// `#[subsystem(no_dispatch, , A | B | C)]`
#[derive(Clone, Debug)]
pub(crate) struct SubSysField {
	/// Name of the field.
	pub(crate) name: Ident,
	/// Generate generic type name for the `AllSubsystems` type
	/// which is also used `#wrapper_message :: #variant` variant
	/// part.
	pub(crate) generic: Ident,
	/// Type to be consumed by the subsystem.
	pub(crate) consumes: Path,
	/// If `no_dispatch` is present, if the message is incoming via
	/// an `extern` `Event`, it will not be dispatched to all subsystems.
	pub(crate) no_dispatch: bool,
	/// If the subsystem implementation is blocking execution and hence
	/// has to be spawned on a separate thread or thread pool.
	pub(crate) blocking: bool,
	/// The subsystem is a work in progress.
	/// Avoids dispatching `Wrapper` type messages, but generates the variants.
	/// Does not require the subsystem to be instantiated with the builder pattern.
	pub(crate) wip: bool,
}

fn try_type_to_path(ty: Type, span: Span) -> Result<Path> {
	match ty {
		Type::Path(path) => Ok(path.path),
		_ => Err(Error::new(span, "Type must be a path expression.")),
	}
}

macro_rules! extract_variant {
	($unique:expr, $variant:ident ; default = $fallback:expr) => {
		extract_variant!($unique, $variant)
			.unwrap_or_else(|| { $fallback })
	};
	($unique:expr, $variant:ident ; err = $err:expr) => {
		extract_variant!($unique, $variant)
			.ok_or_else(|| {
				Error::new(Span::call_site(), $err)
			})
	};
	($unique:expr, $variant:ident) => {
		$unique.values()
			.find_map(|item| {
				if let SubSysAttrItem:: $variant ( _ ) = item {
					Some(true)
				} else {
					None
				}
			})
	};
}


pub(crate) struct SubSystemTags {
	#[allow(dead_code)]
	pub(crate) attrs: Vec<Attribute>,
	#[allow(dead_code)]
	pub(crate) no_dispatch: bool,
	/// The subsystem is in progress, only generate the `Wrapper` variant, but do not forward messages
	/// and also not include the subsystem in the list of subsystems.
	pub(crate) wip: bool,
	pub(crate) blocking: bool,
	pub(crate) consumes: Path,
}

impl Parse for SubSystemTags {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let attrs = Attribute::parse_outer(input)?;

		let input = input;
		let content;
		let _ = syn::parenthesized!(content in input);

		let mut items = Punctuated::new();
		while let Ok(tag) = content.call(SubSysAttrItem::parse) {
			items.push_value(tag);
			items.push_punct(content.call(<Token![,]>::parse)?);
		}

		assert!(items.empty_or_trailing(), "Always followed by the message type to consume. qed");

		let consumes = content.parse::<Path>()?;

		let mut unique = HashMap::<std::mem::Discriminant<SubSysAttrItem>, SubSysAttrItem, RandomState>::default();
		for item in items {
			if let Some(first) = unique.insert(std::mem::discriminant(&item), item.clone()) {
				let mut e = Error::new(item.span(), format!("Duplicate definition of subsystem attribute found"));
				e.combine(Error::new(first.span(), "previously defined here."));
				return Err(e);
			}
		}

		let no_dispatch = extract_variant!(unique, NoDispatch; default = false);
		let blocking = extract_variant!(unique, Blocking; default = false);
		let wip = extract_variant!(unique, Wip; default = false);

		Ok(Self { attrs, no_dispatch, blocking, consumes, wip })
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
	/// Where the support crate `::polkadot_overseer_gen` lives.
	pub(crate) support_crate_name: TokenStream,

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
	pub(crate) fn support_crate_name(&self) -> &TokenStream {
		&self.support_crate_name
	}

	pub(crate) fn variant_names(&self) -> Vec<Ident> {
		self.subsystems
			.iter()
			.map(|ssf| ssf.generic.clone())
			.collect::<Vec<_>>()
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

	/// Generic types per subsystem, as defined by the user.
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

		// The types of `#[subsystem(..)]` annotated fields
		// have to be unique, since they are used as generics
		// for the builder pattern besides other places.
		let mut unique_subsystem_idents = HashSet::<Ident>::new();
		for Field { attrs, vis, ident, ty, .. } in fields.named.into_iter() {
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
				let variant: SubSystemTags = syn::parse2(attr_tokens.clone())?;
				consumes_paths.push(variant.consumes);

				let field_ty = try_type_to_path(ty, span)?;
				let generic = field_ty.get_ident().ok_or_else(|| Error::new(field_ty.span(), "Must be an identifier, not a path."))?.clone();
				if let Some(previous) = unique_subsystem_idents.get(&generic) {
					let mut e = Error::new(generic.span(), format!("Duplicate subsystem names `{}`", generic));
					e.combine(Error::new(previous.span(), "previously defined here."));
					return Err(e)
				}
				unique_subsystem_idents.insert(generic.clone());

				subsystems.push(SubSysField {
					name: ident,
					generic,
					consumes: consumes_paths[0].clone(),
					no_dispatch: variant.no_dispatch,
					wip: variant.wip,
					blocking: variant.blocking,
				});
			} else {
				let field_ty = try_type_to_path(ty, ident.span())?;
				let generic = field_ty.get_ident().map(|ident| baggage_generics.contains(ident)).unwrap_or_default();
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
