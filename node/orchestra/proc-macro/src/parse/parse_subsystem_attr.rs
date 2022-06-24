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
	Error, Ident, Path, Result, Token,
};

#[derive(Clone, Debug)]
enum SubsystemAttrItem {
	/// Error type provided by the user.
	Error { tag: kw::error, eq_token: Token![=], value: Path },
	/// For which slot in the orchestra this should be plugged.
	///
	/// The subsystem implementation can and should have a different name
	/// from the declared parameter type in the orchestra.
	Subsystem { tag: Option<kw::subsystem>, eq_token: Option<Token![=]>, value: Ident },
	/// The prefix to apply when a subsystem is implemented in a different file/crate
	/// than the orchestra itself.
	///
	/// Important for `#[subsystem(..)]` to reference the traits correctly.
	TraitPrefix { tag: kw::prefix, eq_token: Token![=], value: Path },
}

impl ToTokens for SubsystemAttrItem {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			Self::TraitPrefix { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::Error { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
			Self::Subsystem { tag, eq_token, value } => {
				quote! { #tag #eq_token, #value }
			},
		};
		tokens.extend(ts.into_iter());
	}
}

impl Parse for SubsystemAttrItem {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let lookahead = input.lookahead1();
		if lookahead.peek(kw::error) {
			Ok(SubsystemAttrItem::Error {
				tag: input.parse::<kw::error>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::prefix) {
			Ok(SubsystemAttrItem::TraitPrefix {
				tag: input.parse::<kw::prefix>()?,
				eq_token: input.parse()?,
				value: input.parse()?,
			})
		} else if lookahead.peek(kw::subsystem) {
			Ok(SubsystemAttrItem::Subsystem {
				tag: Some(input.parse::<kw::subsystem>()?),
				eq_token: Some(input.parse()?),
				value: input.parse()?,
			})
		} else {
			Ok(SubsystemAttrItem::Subsystem { tag: None, eq_token: None, value: input.parse()? })
		}
	}
}

/// Attribute arguments `$args` in `#[subsystem( $args )]`.
#[derive(Clone, Debug)]
pub(crate) struct SubsystemAttrArgs {
	span: Span,
	pub(crate) error_path: Option<Path>,
	pub(crate) subsystem_ident: Ident,
	pub(crate) trait_prefix_path: Option<Path>,
}

impl Spanned for SubsystemAttrArgs {
	fn span(&self) -> Span {
		self.span.clone()
	}
}

macro_rules! extract_variant {
	($unique:expr, $variant:ident ; default = $fallback:expr) => {
		extract_variant!($unique, $variant).unwrap_or_else(|| $fallback)
	};
	($unique:expr, $variant:ident ; err = $err:expr) => {
		extract_variant!($unique, $variant).ok_or_else(|| Error::new(Span::call_site(), $err))
	};
	($unique:expr, $variant:ident) => {
		$unique.values().find_map(|item| match item {
			SubsystemAttrItem::$variant { value, .. } => Some(value.clone()),
			_ => None,
		})
	};
}

impl Parse for SubsystemAttrArgs {
	fn parse(input: &ParseBuffer) -> Result<Self> {
		let span = input.span();
		let items: Punctuated<SubsystemAttrItem, Token![,]> =
			input.parse_terminated(SubsystemAttrItem::parse)?;

		let mut unique = HashMap::<
			std::mem::Discriminant<SubsystemAttrItem>,
			SubsystemAttrItem,
			RandomState,
		>::default();
		for item in items {
			if let Some(first) = unique.insert(std::mem::discriminant(&item), item.clone()) {
				let mut e = Error::new(
					item.span(),
					format!("Duplicate definition of subsystem generation type found"),
				);
				e.combine(Error::new(first.span(), "previously defined here."));
				return Err(e)
			}
		}
		let error_path = extract_variant!(unique, Error);
		let subsystem_ident = extract_variant!(unique, Subsystem; err = "Must annotate the identical orchestra error type via `subsystem=..` or plainly as `Subsystem` as specified in the orchestra declaration.")?;
		let trait_prefix_path = extract_variant!(unique, TraitPrefix);
		Ok(SubsystemAttrArgs { span, error_path, subsystem_ident, trait_prefix_path })
	}
}
