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

use proc_macro2::Span;
use std::collections::{hash_map::RandomState, HashMap};
use syn::parse::Parse;
use syn::parse::ParseBuffer;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::Error;
use syn::Ident;
use syn::LitInt;
use syn::Path;
use syn::Result;
use syn::Token;

#[derive(Clone, Debug)]
enum AttrItem {
	ExternEventType(Path),
	ExternNetworkType(Path),
	ExternOverseerSignalType(Path),
	ExternErrorType(Path),
	OutgoingType(Path),
	MessageWrapperName(Ident),
	SignalChannelCapacity(LitInt),
	MessageChannelCapacity(LitInt),
}

impl Spanned for AttrItem {
	fn span(&self) -> Span {
		match self {
			AttrItem::ExternEventType(x) => x.span(),
			AttrItem::ExternNetworkType(x) => x.span(),
			AttrItem::ExternOverseerSignalType(x) => x.span(),
			AttrItem::ExternErrorType(x) => x.span(),
			AttrItem::OutgoingType(x) => x.span(),
			AttrItem::MessageWrapperName(x) => x.span(),
			AttrItem::SignalChannelCapacity(x) => x.span(),
			AttrItem::MessageChannelCapacity(x) => x.span(),
		}
	}
}

const TAG_EXT_EVENT_TY: &str = "event";
const TAG_EXT_SIGNAL_TY: &str = "signal";
const TAG_EXT_ERROR_TY: &str = "error";
const TAG_EXT_NETWORK_TY: &str = "network";
const TAG_OUTGOING_TY: &str = "outgoing";
const TAG_GEN_TY: &str = "gen";
const TAG_SIGNAL_CAPACITY: &str = "signal_capacity";
const TAG_MESSAGE_CAPACITY: &str = "message_capacity";

impl AttrItem {
	fn key(&self) -> &'static str {
		match self {
			AttrItem::ExternEventType(_) => TAG_EXT_EVENT_TY,
			AttrItem::ExternOverseerSignalType(_) => TAG_EXT_SIGNAL_TY,
			AttrItem::ExternErrorType(_) => TAG_EXT_ERROR_TY,
			AttrItem::ExternNetworkType(_) => TAG_EXT_NETWORK_TY,
			AttrItem::OutgoingType(_) => TAG_OUTGOING_TY,
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
		} else if key == TAG_EXT_NETWORK_TY {
			let path = input.parse::<Path>()?;
			AttrItem::ExternNetworkType(path)
		} else if key == TAG_OUTGOING_TY {
			let path = input.parse::<Path>()?;
			AttrItem::OutgoingType(path)
		} else if key == TAG_EXT_EVENT_TY {
			let path = input.parse::<Path>()?;
			AttrItem::ExternEventType(path)
		} else if key == TAG_EXT_ERROR_TY {
			let path = input.parse::<Path>()?;
			AttrItem::ExternErrorType(path)
		} else if key == TAG_GEN_TY {
			let wrapper_message = input.parse::<Ident>()?;
			AttrItem::MessageWrapperName(wrapper_message)
		} else if key == TAG_SIGNAL_CAPACITY {
			let value = input.parse::<LitInt>()?;
			AttrItem::SignalChannelCapacity(value)
		} else if key == TAG_MESSAGE_CAPACITY {
			let value = input.parse::<LitInt>()?;
			AttrItem::MessageChannelCapacity(value)
		} else {
			return Err(Error::new(span, "Expected one of `gen`, `signal_capacity`, or `message_capacity`."));
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
	/// A external subsystem that both consumes and produces messages
	/// but is not part of the band of subsystems, it's a mere proxy
	/// to another entity that consumes/produces messages.
	pub(crate) extern_network_ty: Option<Path>,
	pub(crate) outgoing_ty: Option<Path>,
	pub(crate) signal_channel_capacity: usize,
	pub(crate) message_channel_capacity: usize,
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
				return Err(e);
			}
		}

		let signal_channel_capacity = if let Some(item) = unique.remove(TAG_SIGNAL_CAPACITY) {
			if let AttrItem::SignalChannelCapacity(lit) = item {
				lit.base10_parse::<usize>()?
			} else {
				unreachable!()
			}
		} else {
			64_usize
		};

		let message_channel_capacity = if let Some(item) = unique.remove(TAG_MESSAGE_CAPACITY) {
			if let AttrItem::MessageChannelCapacity(lit) = item {
				lit.base10_parse::<usize>()?
			} else {
				unreachable!()
			}
		} else {
			1024_usize
		};
		let extern_error_ty = unique
			.remove(TAG_EXT_ERROR_TY)
			.map(|x| if let AttrItem::ExternErrorType(x) = x { x.clone() } else { unreachable!() })
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the overseer signals type via `{}=..`.", TAG_EXT_ERROR_TY))
			})?;

		let extern_signal_ty = unique
			.remove(TAG_EXT_SIGNAL_TY)
			.map(|x| if let AttrItem::ExternOverseerSignalType(x) = x { x.clone() } else { unreachable!() })
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the overseer signals type via `{}=..`.", TAG_EXT_SIGNAL_TY))
			})?;

		let extern_event_ty = unique
			.remove(TAG_EXT_EVENT_TY)
			.map(|x| if let AttrItem::ExternEventType(x) = x { x.clone() } else { unreachable!() })
			.ok_or_else(|| {
				Error::new(span, format!("Must declare the external network type via `{}=..`.", TAG_EXT_NETWORK_TY))
			})?;

		let extern_network_ty = unique
			.remove(TAG_EXT_NETWORK_TY)
			.map(|x| if let AttrItem::ExternNetworkType(x) = x { x.clone() } else { unreachable!() });

		let outgoing_ty = unique
			.remove(TAG_OUTGOING_TY)
			.map(|x| if let AttrItem::OutgoingType(x) = x { x.clone() } else { unreachable!() });

		let message_wrapper = unique
			.remove(TAG_GEN_TY)
			.map(|x| if let AttrItem::MessageWrapperName(x) = x { x.clone() } else { unreachable!() })
			.ok_or_else(|| Error::new(span, format!("Must declare the generated type via `{}=..`.", TAG_GEN_TY)))?;

		if !unique.is_empty() {
			let v = unique.into_iter().map(|(tag, _attr)| -> String { format!("`{}`", tag) }).collect::<Vec<_>>();
			let s = v.join(", ");

			return Err(Error::new(span, format!("Found unknown arguments to the overseer macro {}.", s)));
		}

		Ok(AttrArgs {
			signal_channel_capacity,
			message_channel_capacity,
			extern_event_ty,
			extern_signal_ty,
			extern_error_ty,
			extern_network_ty,
			outgoing_ty,
			message_wrapper,
		})
	}
}
