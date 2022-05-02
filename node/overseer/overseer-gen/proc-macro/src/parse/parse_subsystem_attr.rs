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
	ExternErrorType {
		tag: kw::error,
		eq_token: Token![=],
		value: Path,
	},
	Subsystem {
		tag: Option<kw::subsystem>,
		eq_token: Option<Token![=]>,
		value: Ident,
	},
}

impl ToTokens for SubsystemAttrItem {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		let ts = match self {
			Self::ExternErrorType { tag, eq_token, value } => {
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
			Ok(SubsystemAttrItem::ExternErrorType {
				tag: input.parse::<kw::error>()?,
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
	pub(crate) extern_error_ty: Path,
	pub(crate) subsystem_ident: Ident,
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
		let extern_error_ty = extract_variant!(unique, ExternErrorType; err = "Must annotate the identical overseer error type via `error=..`.")?;
		let subsystem_ident = extract_variant!(unique, Subsystem; err = "Must annotate the identical overseer error type via `subsystem=..` or plainly as `Subsystem` as specified in the overseer declaration.")?;
		Ok(SubsystemAttrArgs { span, extern_error_ty, subsystem_ident })
	}
}
