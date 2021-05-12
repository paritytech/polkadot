use proc_macro2::Span;
use std::collections::{HashMap, hash_map::RandomState};
use syn::Path;
use syn::Error;
use syn::Ident;
use syn::LitInt;
use syn::parse::Parse;
use syn::parse::ParseBuffer;
use syn::punctuated::Punctuated;
use syn::Result;
use syn::spanned::Spanned;
use syn::Token;


#[derive(Clone, Debug)]
enum AttrItem {
	ExternEventType(Path),
	ExternOverseerSignalType(Path),
	ExternErrorType(Path),
	MessageWrapperName(Ident),
	SignalChannelCapacity(LitInt),
	MessageChannelCapacity(LitInt),
}

impl Spanned for AttrItem {
	fn span(&self) -> Span {
		match self {
			AttrItem::ExternEventType(x) => x.span(),
			AttrItem::ExternOverseerSignalType(x) => x.span(),
			AttrItem::ExternErrorType(x) => x.span(),
			AttrItem::MessageWrapperName(x) => x.span(),
			AttrItem::SignalChannelCapacity(x) => x.span(),
			AttrItem::MessageChannelCapacity(x) => x.span(),
		}
	}
}

const TAG_EXT_EVENT_TY: &str = "event";
const TAG_EXT_SIGNAL_TY: &str = "signal";
const TAG_EXT_ERROR_TY: &str = "error";
const TAG_GEN_TY: &str = "gen";
const TAG_SIGNAL_CAPACITY: &str = "signal_capacity";
const TAG_MESSAGE_CAPACITY: &str = "message_capacity";

impl AttrItem {
	fn key(&self) -> &'static str {
		match self {
			AttrItem::ExternEventType(_) => TAG_EXT_EVENT_TY,
			AttrItem::ExternOverseerSignalType(_) => TAG_EXT_SIGNAL_TY,
			AttrItem::ExternErrorType(_) => TAG_EXT_ERROR_TY,
			AttrItem::MessageWrapperName(_) => TAG_GEN_TY,
			AttrItem::SignalChannelCapacity(_) => TAG_SIGNAL_CAPACITY,
			AttrItem::MessageChannelCapacity(_) => TAG_MESSAGE_CAPACITY,
		}
	}
}

impl Parse for AttrItem {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let key = input.parse::<Ident>()?;
		let span = key.span();
		let _ = input.parse::<Token![=]>()?;
		Ok(if key == TAG_EXT_SIGNAL_TY {
			let path = input.parse::<Path>()?;
			AttrItem::ExternOverseerSignalType(path)
		} else if  key == TAG_EXT_EVENT_TY {
			let path = input.parse::<Path>()?;
			AttrItem::ExternEventType(path)
		} else if  key == TAG_EXT_ERROR_TY {
			let path = input.parse::<Path>()?;
			AttrItem::ExternErrorType(path)
		} else if  key == TAG_GEN_TY {
			let wrapper_message = input.parse::<Ident>()?;
			AttrItem::MessageWrapperName(wrapper_message)
		} else if key == TAG_SIGNAL_CAPACITY {
			let value = input.parse::<LitInt>()?;
			AttrItem::SignalChannelCapacity(value)
		} else if key == TAG_MESSAGE_CAPACITY {
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
	pub(crate) extern_signal_ty: Path,
	pub(crate) extern_error_ty: Path,
	pub(crate) signal_channel_capacity: u64,
	pub(crate) message_channel_capacity: u64,
}

impl Parse for AttrArgs {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let span = input.span();
		let items: Punctuated<AttrItem, Token![,]> = input.parse_terminated(AttrItem::parse)?;

		let mut unique = HashMap::<&str, AttrItem, RandomState>::default();
		for item in items {
			if let Some(first) = unique.insert(item.key(), item.clone()) {
				let mut e = Error::new(item.span(), format!("Duplicate definition of `{}` found", item.key()));
				e.combine(Error::new(first.span(), "previously defined here."));
				return Err(e)
			}
		}

		let signal_channel_capacity = if let Some(item) = unique.remove(TAG_SIGNAL_CAPACITY) {
			if let AttrItem::SignalChannelCapacity(lit) = item {
				lit.base10_parse::<u64>()?
			} else {
				unreachable!()
			}
		} else {
			64
		};

		let message_channel_capacity = if let Some(item) = unique.remove(TAG_MESSAGE_CAPACITY) {
			if let AttrItem::MessageChannelCapacity(lit) = item {
				lit.base10_parse::<u64>()?
			} else {
				unreachable!()
			}
		} else {
			1024
		};
		let extern_error_ty = unique.remove(TAG_EXT_ERROR_TY)
			.map(|x| if let AttrItem::ExternErrorType(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the overseer signals type via `{}=..`.", TAG_EXT_ERROR_TY))
			})?;

		let extern_signal_ty = unique.remove(TAG_EXT_SIGNAL_TY)
			.map(|x| if let AttrItem::ExternOverseerSignalType(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the overseer signals type via `{}=..`.", TAG_EXT_SIGNAL_TY))
			})?;

		let extern_event_ty = unique.remove(TAG_EXT_EVENT_TY)
			.map(|x| if let AttrItem::ExternEventType(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the external event type via `{}=..`.", TAG_EXT_EVENT_TY))

			})?;

		let message_wrapper = unique.remove(TAG_GEN_TY)
			.map(|x| if let AttrItem::MessageWrapperName(x) = x { x.clone() } else { unreachable!() } )
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the generated type via `{}=..`.", TAG_GEN_TY))
			})?;

		if !unique.is_empty() {
			let v = unique.into_iter().map(|(tag, _attr)| -> String {
				format!("`{}`", tag)
			}).collect::<Vec<_>>();
			let s = v.join(", ");

			return Err(Error::new(span, format!("Found unknown arguments to the overseer macro {}.", s)))
		}

		Ok(AttrArgs {
			signal_channel_capacity,
			message_channel_capacity,
			extern_event_ty,
			extern_signal_ty,
			extern_error_ty,
			message_wrapper,
		})
	}
}
