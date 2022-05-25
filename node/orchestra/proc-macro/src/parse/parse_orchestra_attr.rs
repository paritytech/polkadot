// Copyright (C) 2022 Parity Technologies (UK) Ltd.
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

use super::kw;
use proc_macro2::Span;
use quote::{quote, ToTokens};
use std::collections::{hash_map::RandomState, HashMap};
use syn::{
	parse::{Parse, ParseBuffer},
	punctuated::Punctuated,
	spanned::Spanned,
	Error, Ident, LitInt, Path, Result, Token,
};

#[derive(Clone, Debug)]
enum OrchestraAttrItem {
	ExternEventType { tag: kw::event, eq_token: Token![=], value: Path },
	ExternOrchestraSignalType { tag: kw::signal, eq_token: Token![=], value: Path },
	ExternErrorType { tag: kw::error, eq_token: Token![=], value: Path },
	OutgoingType { tag: kw::outgoing, eq_token: Token![=], value: Path },
	MessageWrapperName { tag: kw::gen, eq_token: Token![=], value: Ident },
	SignalChannelCapacity { tag: kw::signal_capacity, eq_token: Token![=], value: usize },
	MessageChannelCapacity { tag: kw::message_capacity, eq_token: Token![=], value: usize },
}

impl ToTokens for OrchestraAttrItem {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			Self::ExternEventType { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::ExternOrchestraSignalType { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::ExternErrorType { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::OutgoingType { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::MessageWrapperName { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::SignalChannelCapacity { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::MessageChannelCapacity { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
		};
		tokens.extend(ts.into_iter());
	}
}

impl Parse for OrchestraAttrItem {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let lookahead = input.lookahead1();
		if lookahead.peek(kw::event) {
			Ok(OrchestraAttrItem::ExternEventType {
				tag: input.parse::<kw::event>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::signal) {
			Ok(OrchestraAttrItem::ExternOrchestraSignalType {
				tag: input.parse::<kw::signal>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::error) {
			Ok(OrchestraAttrItem::ExternErrorType {
				tag: input.parse::<kw::error>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::outgoing) {
			Ok(OrchestraAttrItem::OutgoingType {
				tag: input.parse::<kw::outgoing>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::gen) {
			Ok(OrchestraAttrItem::MessageWrapperName {
				tag: input.parse::<kw::gen>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::signal_capacity) {
			Ok(OrchestraAttrItem::SignalChannelCapacity {
				tag: input.parse::<kw::signal_capacity>()?,
				eq_token: input.parse()?,
				value: input.parse::<LitInt>()?.base10_parse::<usize>()?,
			})
		} else if lookahead.peek(kw::message_capacity) {
			Ok(OrchestraAttrItem::MessageChannelCapacity {
				tag: input.parse::<kw::message_capacity>()?,
				eq_token: input.parse()?,
				value: input.parse::<LitInt>()?.base10_parse::<usize>()?,
			})
		} else {
			Err(lookahead.error())
		}
	}
}

/// Attribute arguments
#[derive(Clone, Debug)]
pub(crate) struct OrchestraAttrArgs {
	pub(crate) message_wrapper: Ident,
	pub(crate) extern_event_ty: Path,
	pub(crate) extern_signal_ty: Path,
	pub(crate) extern_error_ty: Path,
	pub(crate) outgoing_ty: Option<Path>,
	pub(crate) signal_channel_capacity: usize,
	pub(crate) message_channel_capacity: usize,
}

macro_rules! extract_variant {
	($unique:expr, $variant:ident ; default = $fallback:expr) => {
		extract_variant!($unique, $variant).unwrap_or_else(|| $fallback)
	};
	($unique:expr, $variant:ident ; err = $err:expr) => {
		extract_variant!($unique, $variant).ok_or_else(|| Error::new(Span::call_site(), $err))
	};
	($unique:expr, $variant:ident) => {
		$unique.values().find_map(|item| {
			if let OrchestraAttrItem::$variant { value, .. } = item {
				Some(value.clone())
			} else {
				None
			}
		})
	};
}

impl Parse for OrchestraAttrArgs {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let items: Punctuated<OrchestraAttrItem, Token![,]> =
			input.parse_terminated(OrchestraAttrItem::parse)?;

		let mut unique = HashMap::<
			std::mem::Discriminant<OrchestraAttrItem>,
			OrchestraAttrItem,
			RandomState,
		>::default();
		for item in items {
			if let Some(first) = unique.insert(std::mem::discriminant(&item), item.clone()) {
				let mut e = Error::new(
					item.span(),
					format!("Duplicate definition of orchestra generation type found"),
				);
				e.combine(Error::new(first.span(), "previously defined here."));
				return Err(e)
			}
		}

		let signal_channel_capacity =
			extract_variant!(unique, SignalChannelCapacity; default = 64_usize);
		let message_channel_capacity =
			extract_variant!(unique, MessageChannelCapacity; default = 1024_usize);

		let error = extract_variant!(unique, ExternErrorType; err = "Must declare the orchestra error type via `error=..`.")?;
		let event = extract_variant!(unique, ExternEventType; err = "Must declare the orchestra event type via `event=..`.")?;
		let signal = extract_variant!(unique, ExternOrchestraSignalType; err = "Must declare the orchestra signal type via `signal=..`.")?;
		let message_wrapper = extract_variant!(unique, MessageWrapperName; err = "Must declare the orchestra generated wrapping message type via `gen=..`.")?;
		let outgoing = extract_variant!(unique, OutgoingType);

		Ok(OrchestraAttrArgs {
			signal_channel_capacity,
			message_channel_capacity,
			extern_event_ty: event,
			extern_signal_ty: signal,
			extern_error_ty: error,
			outgoing_ty: outgoing,
			message_wrapper,
		})
	}
}
