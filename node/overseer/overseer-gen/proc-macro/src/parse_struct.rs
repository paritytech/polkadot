use proc_macro2::Span;
use std::collections::{HashSet, hash_map::RandomState};
use syn::{AttrStyle, Path};
use syn::Attribute;
use syn::Field;
use syn::FieldsNamed;
use syn::Ident;
use syn::parse::Parse;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::Token;
use syn::Type;
use syn::{parse2, ItemStruct, Error, GenericParam, Result};
use syn::parse::ParseStream;

/// A field of the struct annotated with
/// `#[subsystem(no_dispatch, , A | B | C)]`
#[derive(Clone, Debug)]
pub(crate) struct SubSysField {
	/// Name of the field.
	pub(crate) name: Ident,
	/// Generate generic type name for the `AllSubsystems` type.
	pub(crate) generic: Ident,
	/// Type of the subsystem.
	pub(crate) ty: Ident,
	/// Type to be consumed by the subsystem.
	pub(crate) consumes: Ident,
	/// If `no_dispatch` is present, if the message is incomming via
	/// an extern `Event`, it will not be dispatched to all subsystems.
	pub(crate) no_dispatch: bool,
	/// If the subsystem imlementation is blocking execution and hence
	/// has to be spawned on a separate thread or thread pool.
	pub(crate) blocking: bool,
}

fn try_type_to_ident(ty: Type, span: Span) -> Result<Ident> {
	match ty {
		Type::Path(path) => {
			path.path.get_ident().cloned().ok_or_else(|| Error::new(span, "Expected an identifier, but got a path."))
		}
		_ => Err(Error::new(span, "Type must be a path expression.")),
	}
}

pub(crate) struct SubSystemTag {
	#[allow(dead_code)]
	pub(crate) attrs: Vec<Attribute>,
	#[allow(dead_code)]
	pub(crate) no_dispatch: bool,
	pub(crate) blocking: bool,
	pub(crate) consumes: Punctuated<Ident, Token![|]>,
}

impl Parse for SubSystemTag {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let attrs = Attribute::parse_outer(input)?;

		let input = dbg!(input);
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
			if ident != "no_dispatch" && ident != "blocking" {
				return Err(Error::new(ident.span(), "Allowed tags are only `no_dispatch` or `blocking`."))
			}
			if !unique.insert(ident.to_string()) {
				return Err(Error::new(ident.span(), "Found duplicate tag."))
			}
		}
		let no_dispatch = unique.take("no_dispatch").is_some();
		let blocking = unique.take("blocking").is_some();

		let consumes = content.parse_terminated(Ident::parse)?;

		Ok(Self {
			attrs,
			no_dispatch,
			blocking,
			consumes,
		})
	}
}


/// Fields that are _not_ subsystems.
#[derive(Debug, Clone)]
pub(crate) struct BaggageField {
	pub(crate) field_name: Ident,
	pub(crate) field_ty: Ident,
	pub(crate) generic: bool,
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
	pub(crate) message_channel_capacity: u64,
	/// Size of the bounded signal channel.
	pub(crate) signal_channel_capacity: u64,

	/// Signals to be sent, sparse information that is used intermittendly.
	pub(crate) extern_signal_ty: Path,

	/// Incoming event type from the outer world, commonly from the network.
	pub(crate) extern_event_ty: Path,

	/// Incoming event type from the outer world, commonly from the network.
	pub(crate) extern_error_ty: Path,
}

impl OverseerInfo {
	pub(crate) fn subsystems(&self) -> &[SubSysField] {
		self.subsystems.as_slice()
	}

	pub(crate) fn subsystem_names(&self) -> Vec<Ident> {
		self.subsystems.iter().map(|ssf| ssf.name.clone()).collect::<Vec<_>>()
	}

	#[allow(dead_code)]
	// FIXME use as the defaults
	pub(crate) fn subsystem_types(&self) -> Vec<Ident> {
		self.subsystems.iter().map(|ssf| ssf.ty.clone()).collect::<Vec<_>>()
	}

	pub(crate) fn baggage_names(&self) -> Vec<Ident> {
		self.baggage.iter().map(|bag| bag.field_name.clone()).collect::<Vec<_>>()
	}
	pub(crate) fn baggage_types(&self) -> Vec<Ident> {
		self.baggage.iter().map(|bag| bag.field_ty.clone()).collect::<Vec<_>>()
	}

	/// Generic types per subsystem, in the form `Sub#N`.
	pub(crate) fn builder_generic_types(&self) -> Vec<Ident> {
		self.subsystems.iter().map(|sff| sff.generic.clone()).collect::<Vec<_>>()
	}

	pub(crate) fn baggage_generic_types(&self) -> Vec<Ident> {
		self.baggage.iter().filter(|bag| bag.generic).map(|bag| dbg!(bag.field_ty.clone())).collect::<Vec<_>>()
	}

	pub(crate) fn channel_names(&self, suffix: &'static str) -> Vec<Ident> {
		self.subsystems.iter()
		.map(|ssf| Ident::new(&(ssf.name.to_string() + suffix), ssf.name.span()))
		.collect::<Vec<_>>()
	}

	pub(crate) fn consumes(&self) -> Vec<Ident> {
		self.subsystems.iter()
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
		for (idx, Field { attrs, vis: _, ident, ty, .. }) in fields.named.into_iter().enumerate() {
			let mut consumes = attrs.iter().filter(|attr| attr.style == AttrStyle::Outer).filter_map(|attr| {
				let span = attr.path.span();
				attr.path.get_ident().filter(|ident| *ident == "subsystem").map(move |_ident| {
					let attr_tokens = attr.tokens.clone();
					(attr_tokens, span)
				})
			});
			let ident = ident.ok_or_else(|| {
				Error::new(ty.span(), "Missing identifier for member. BUG")
			})?;

			if let Some((attr_tokens, span)) = consumes.next() {
				if let Some((_attr_tokens2, span2)) = consumes.next() {
					return Err({
						let mut err = Error::new(span, "The first subsystem annotation is at");
						err.combine(
								Error::new(span2, "but another here for the same field.")
							);
						err
					})
				}
				let mut consumes_idents = Vec::with_capacity(attrs.len());
				let attr_tokens = dbg!(attr_tokens.clone());
				let variant: SubSystemTag = syn::parse2(attr_tokens.clone())?;
				if variant.consumes.len() != 1 {
					return Err(Error::new(attr_tokens.span(), "Exactly one message can be consumed per subsystem."))
				}
				consumes_idents.extend(variant.consumes.into_iter());


				if consumes_idents.is_empty() {
					return Err(
						Error::new(span, "Subsystem must consume at least one message")
					)
				}

				subsystems.push(SubSysField {
					name: ident,
					generic: Ident::new(format!("Sub{}", idx).as_str(), span),
					ty: try_type_to_ident(ty, span)?,
					consumes: consumes_idents[0].clone(),
					no_dispatch: variant.no_dispatch,
					blocking: variant.blocking,
				});
			} else {
				let field_ty = try_type_to_ident(ty, ident.span())?;
				baggage.push(BaggageField {
					field_name: ident,
					generic: !baggage_generics.contains(&field_ty),
					field_ty,
				});
			}
		}
		Ok( Self { name, subsystems, baggage })
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
			syn::Fields::Unit => Err(Error::new(ds.fields.span(), "Must be a struct with named fields. Not an unit struct.")),
			syn::Fields::Unnamed(unnamed) => {
				Err(Error::new(unnamed.span(), "Must be a struct with named fields. Not an unnamed fields struct."))
			}
		}
	}
}
