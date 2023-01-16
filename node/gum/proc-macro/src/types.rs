// Copyright 2022 Parity Technologies (UK) Ltd.
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

use super::*;

use syn::{
	parse::{Parse, ParseStream},
	Token,
};

pub(crate) mod kw {
	syn::custom_keyword!(target);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
	kw: kw::target,
	colon: Token![:],
	expr: syn::Expr,
}

impl Parse for Target {
	fn parse(input: ParseStream) -> Result<Self> {
		Ok(Self { kw: input.parse()?, colon: input.parse()?, expr: input.parse()? })
	}
}

impl ToTokens for Target {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		let kw = &self.kw;
		let colon = &self.colon;
		let expr = &self.expr;
		tokens.extend(quote! {
			#kw #colon #expr
		})
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatMarker {
	Questionmark(Token![?]),
	Percentage(Token![%]),
	None,
}

impl Parse for FormatMarker {
	fn parse(input: ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();
		if lookahead.peek(Token![?]) {
			input.parse().map(Self::Questionmark)
		} else if lookahead.peek(Token![%]) {
			input.parse().map(Self::Percentage)
		} else {
			Ok(Self::None)
		}
	}
}

impl ToTokens for FormatMarker {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		tokens.extend(match self {
			Self::Percentage(p) => p.to_token_stream(),
			Self::Questionmark(q) => q.to_token_stream(),
			Self::None => TokenStream::new(),
		})
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValueWithAliasIdent {
	pub alias: Ident,
	pub eq: Token![=],
	pub marker: FormatMarker,
	pub expr: syn::Expr,
}

impl Parse for ValueWithAliasIdent {
	fn parse(input: ParseStream) -> Result<Self> {
		Ok(Self {
			alias: input.parse()?,
			eq: input.parse()?,
			marker: input.parse()?,
			expr: input.parse()?,
		})
	}
}

impl ToTokens for ValueWithAliasIdent {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		let alias = &self.alias;
		let eq = &self.eq;
		let marker = &self.marker;
		let expr = &self.expr;
		tokens.extend(quote! {
			#alias #eq #marker #expr
		})
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]

pub(crate) struct ValueWithFormatMarker {
	pub marker: FormatMarker,
	pub ident: Ident,
	pub dot: Option<Token![.]>,
	pub inner: Punctuated<syn::Member, Token![.]>,
}

impl Parse for ValueWithFormatMarker {
	fn parse(input: ParseStream) -> Result<Self> {
		let marker = input.parse::<FormatMarker>()?;
		let ident = input.parse::<syn::Ident>()?;

		let mut inner = Punctuated::<syn::Member, Token![.]>::new();

		let lookahead = input.lookahead1();
		let dot = if lookahead.peek(Token![.]) {
			let dot = Some(input.parse::<Token![.]>()?);

			loop {
				let member = input.parse::<syn::Member>()?;
				inner.push_value(member);

				let lookahead = input.lookahead1();
				if !lookahead.peek(Token![.]) {
					break
				}

				let token = input.parse::<Token![.]>()?;
				inner.push_punct(token);
			}

			dot
		} else {
			None
		};
		Ok(Self { marker, ident, dot, inner })
	}
}

impl ToTokens for ValueWithFormatMarker {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		let marker = &self.marker;
		let ident = &self.ident;
		let dot = &self.dot;
		let inner = &self.inner;
		tokens.extend(quote! {
			#marker #ident #dot #inner
		})
	}
}

/// A value as passed to the macro, appearing _before_ the format string.
#[derive(Debug, Clone, PartialEq, Eq)]

pub(crate) enum Value {
	Alias(ValueWithAliasIdent),
	Value(ValueWithFormatMarker),
}

impl Value {
	pub fn as_ident(&self) -> &Ident {
		match self {
			Self::Alias(alias) => &alias.alias,
			Self::Value(value) => &value.ident,
		}
	}
}

impl Parse for Value {
	fn parse(input: ParseStream) -> Result<Self> {
		if input.fork().parse::<ValueWithAliasIdent>().is_ok() {
			input.parse().map(Self::Alias)
		} else if input.fork().parse::<ValueWithFormatMarker>().is_ok() {
			input.parse().map(Self::Value)
		} else {
			Err(syn::Error::new(Span::call_site(), "Neither value nor aliased value."))
		}
	}
}

impl ToTokens for Value {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		tokens.extend(match self {
			Self::Alias(alias) => quote! { #alias },
			Self::Value(value) => quote! { #value },
		})
	}
}

/// Defines the token stream consisting of a format string and it's arguments.
///
/// Attention: Currently the correctness of the arguments is not checked as part
/// of the parsing logic.
/// It would be possible to use `parse_fmt_str:2.0.0`
/// to do so and possibly improve the error message here - for the time being
/// it's not clear if this yields any practical benefits, and is hence
/// left for future consideration.
#[derive(Debug, Clone)]
pub(crate) struct FmtGroup {
	pub format_str: syn::LitStr,
	pub maybe_comma: Option<Token![,]>,
	pub rest: TokenStream,
}

impl Parse for FmtGroup {
	fn parse(input: ParseStream) -> Result<Self> {
		let format_str = input
			.parse()
			.map_err(|e| syn::Error::new(e.span(), "Expected format specifier"))?;

		let (maybe_comma, rest) = if input.peek(Token![,]) {
			let comma = input.parse::<Token![,]>()?;
			let rest = input.parse()?;
			(Some(comma), rest)
		} else {
			(None, TokenStream::new())
		};

		if !input.is_empty() {
			return Err(syn::Error::new(input.span(), "Unexpected data, expected closing `)`."))
		}

		Ok(Self { format_str, maybe_comma, rest })
	}
}

impl ToTokens for FmtGroup {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		let format_str = &self.format_str;
		let maybe_comma = &self.maybe_comma;
		let rest = &self.rest;

		tokens.extend(quote! { #format_str #maybe_comma #rest });
	}
}

/// Full set of arguments as provided to the `gum::warn!` call.
#[derive(Debug, Clone)]
pub(crate) struct Args {
	pub target: Option<Target>,
	pub comma: Option<Token![,]>,
	pub values: Punctuated<Value, Token![,]>,
	pub fmt: Option<FmtGroup>,
}

impl Parse for Args {
	fn parse(input: ParseStream) -> Result<Self> {
		let lookahead = input.lookahead1();
		let (target, comma) = if lookahead.peek(kw::target) {
			let target = input.parse()?;
			let comma = input.parse::<Token![,]>()?;
			(Some(target), Some(comma))
		} else {
			(None, None)
		};

		let mut values = Punctuated::new();
		loop {
			if input.fork().parse::<Value>().is_ok() {
				values.push_value(input.parse::<Value>()?);
			} else {
				break
			}
			if input.peek(Token![,]) {
				values.push_punct(input.parse::<Token![,]>()?);
			} else {
				break
			}
		}

		let fmt = if values.empty_or_trailing() && !input.is_empty() {
			let fmt = input.parse::<FmtGroup>()?;
			Some(fmt)
		} else {
			None
		};

		Ok(Self { target, comma, values, fmt })
	}
}

impl ToTokens for Args {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		let target = &self.target;
		let comma = &self.comma;
		let values = &self.values;
		let fmt = &self.fmt;
		tokens.extend(quote! {
			#target #comma #values #fmt
		})
	}
}

/// Support tracing levels, passed to `tracing::event!`
///
/// Note: Not parsed from the input stream, but implicitly defined
/// by the macro name, i.e. `level::debug!` is `Level::Debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
	Error,
	Warn,
	Info,
	Debug,
	Trace,
}

impl ToTokens for Level {
	fn to_tokens(&self, tokens: &mut TokenStream) {
		let span = Span::call_site();
		let variant = match self {
			Self::Error => Ident::new("ERROR", span),
			Self::Warn => Ident::new("WARN", span),
			Self::Info => Ident::new("INFO", span),
			Self::Debug => Ident::new("DEBUG", span),
			Self::Trace => Ident::new("TRACE", span),
		};
		let krate = support_crate();
		tokens.extend(quote! {
			#krate :: Level :: #variant
		})
	}
}
