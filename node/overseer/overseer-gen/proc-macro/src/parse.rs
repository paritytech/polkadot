use proc_macro2::Span;
use std::collections::{HashMap, HashSet, hash_map::RandomState};
use syn::{AttrStyle, Path};
use syn::punctuated::Punctuated;
use syn::parse::Parse;
use syn::token::Paren;
use syn::Token;
use syn::Field;
use syn::FieldsNamed;
use syn::Result;
use syn::Ident;
use syn::spanned::Spanned;
use syn::Error;
use syn::Attribute;
use syn::Type;

use syn::LitInt;
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


use syn::parse::ParseBuffer;

#[derive(Clone, Debug)]
enum AttrItem {
	ExternEventType(Path),
	ExternOverseerSignalType(Path),
	MessageWrapperName(Ident),
	SignalChannelCapacity(LitInt),
	MessageChannelCapacity(LitInt),
}

impl Spanned for AttrItem {
	fn span(&self) -> Span {
		match self {
			AttrItem::ExternEventType(x) => x.span(),
			AttrItem::MessageWrapperName(x) => x.span(),
			AttrItem::SignalChannelCapacity(x) => x.span(),
			AttrItem::MessageChannelCapacity(x) => x.span(),
			AttrItem::ExternOverseerSignalType(x) => x.span(),
		}
	}
}

impl AttrItem {
	fn key(&self) -> &'static str {
		match self {
			AttrItem::ExternEventType(_) => "event",
			AttrItem::ExternOverseerSignalType(_) => "signal",
			AttrItem::MessageWrapperName(_) => "gen",
			AttrItem::SignalChannelCapacity(_) => "signal_capacity",
			AttrItem::MessageChannelCapacity(_) => "message_capacity",
		}
	}
}

impl Parse for AttrItem {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let key = input.parse::<Ident>()?;
		let span = Span::call_site();
		let _ = input.parse::<Token![=]>()?;
		Ok(if key == "signal" {
			let path = input.parse::<Path>()?;
			AttrItem::ExternOverseerSignalType(path)
		} else if  key == "event" {
			let path = input.parse::<Path>()?;
			AttrItem::ExternEventType(path)
		} else if  key == "gen" {
			let wrapper_message = input.parse::<Ident>()?;
			AttrItem::MessageWrapperName(wrapper_message)
		} else if key == "signal_capacity" {
			let value = input.parse::<LitInt>()?;
			AttrItem::SignalChannelCapacity(value)
		} else if key == "message_capacity" {
			let value = input.parse::<LitInt>()?;
			AttrItem::MessageChannelCapacity(value)
		} else {
			return Err(Error::new(span, "Expected one of `gen`, `signal_capacity`, or `message_capacity`."))
		})
	}
}

/// Attribute arguments
#[derive(Clone, Debug)]
pub(crate) struct AttrArgs {
	pub(crate) message_wrapper: Ident,
	pub(crate) extern_event_ty: Path,
	pub(crate) signal_channel_capacity: u64,
	pub(crate) message_channel_capacity: u64,
}

impl Parse for AttrArgs {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let span = Span::call_site();

		let content;
		let _paren = syn::parenthesized!(content in input);
		let items: Punctuated<_, Token![,]> = content.parse_terminated(AttrItem::parse)?;

		let mut unique = HashMap::<&str, AttrItem, RandomState>::default();
		for item in items {
			if let Some(first) = unique.insert(item.key(), item.clone()) {
				let mut e = Error::new(item.span(), "Duplicate definition found");
				e.combine(Error::new(first.span(), "previously defined here."));
				return Err(e)
			}
		}

		let signal_channel_capacity = if let Some(item) = unique.get("signal_capacity") {
			if let AttrItem::SignalChannelCapacity(lit) = item {
				lit.base10_parse::<u64>()?
			} else {
				unreachable!()
			}
		} else {
			64
		};

		let message_channel_capacity = if let Some(item) = unique.get("message_capacity") {
			if let AttrItem::MessageChannelCapacity(lit) = item {
				lit.base10_parse::<u64>()?
			} else {
				unreachable!()
			}
		} else {
			1024
		};
		let extern_signal_ty = unique.get("signal")
			.map(|x| if let AttrItem::ExternOverseerSignalType(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, "Must declare the overseer signals type via `signal=..`.")
			})?;

		let extern_event_ty = unique.get("event")
			.map(|x| if let AttrItem::ExternEventType(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, "Must declare the external event type via `event=..`.")
			})?;

		let message_wrapper = unique.get("gen")
			.map(|x| if let AttrItem::MessageWrapperName(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, "Must declare the generated type via `gen=..`.")
			})?;

		Ok(AttrArgs {
			signal_channel_capacity,
			message_channel_capacity,
			extern_event_ty,
			message_wrapper,
		})
	}
}

pub(crate) struct SubSystemTag {
	#[allow(dead_code)]
	pub(crate) attrs: Vec<Attribute>,
	#[allow(dead_code)]
	pub(crate) paren_token: Paren,
	pub(crate) no_dispatch: bool,
	pub(crate) blocking: bool,
	pub(crate) consumes: Punctuated<Ident, Token![|]>,
}

impl Parse for SubSystemTag {
	fn parse(input: syn::parse::ParseStream) -> Result<Self> {
		let attrs = Attribute::parse_outer(input)?;

		let content;
		let paren_token = syn::parenthesized!(content in input);

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
				return Err(Error::new(ident.span(), "Allowed tags are only `no_dispatch` or `blocking`."))
			}
		}
		let no_dispatch = unique.get("no_dispatch").is_some();
		let blocking = unique.get("blocking").is_some();


		let consumes = content.parse_terminated(Ident::parse)?;

		Ok(Self {
			attrs,
			paren_token,
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

	/// Incoming event type, commonly from the network bridge.
	pub(crate) incoming_event_ty: Ident,
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


	pub(crate) fn subsystem_generic_types(&self) -> Vec<Ident> {
		self.subsystems.iter().map(|sff| sff.generic.clone()).collect::<Vec<_>>()
	}

	pub(crate) fn baggage_generic_types(&self) -> Vec<Ident> {
		self.baggage.iter().filter(|bag| bag.generic).map(|bag| bag.field_ty.clone()).collect::<Vec<_>>()
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


/// Creates a list of generic identifiers used for the subsystems
pub(crate) fn parse_overseer_struct_field(
	baggage_generics: HashSet<Ident>,
	fields: FieldsNamed,
) -> Result<(Vec<SubSysField>, Vec<BaggageField>)> {
	let _span = Span::call_site();
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

			let variant = syn::parse2::<SubSystemTag>(attr_tokens.clone())?;
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
				generic: Ident::new(format!("Sub{}", idx).as_str(), Span::call_site()),
				ty: try_type_to_ident(ty, span)?,
				consumes: consumes_idents[0].clone(),
				no_dispatch: variant.no_dispatch,
				blocking: variant.blocking,
			});
		} else {
			let field_ty = try_type_to_ident(ty, Span::call_site())?;
			baggage.push(BaggageField {
				field_name: ident,
				generic: !baggage_generics.contains(&field_ty),
				field_ty,
			});
		}
	}
	Ok((subsystems, baggage))
}
