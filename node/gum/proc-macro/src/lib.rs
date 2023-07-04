// Copyright (C) Parity Technologies (UK) Ltd.
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

#![deny(unused_crate_dependencies)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro)]

//! Generative part of `tracing-gum`. See `tracing-gum` for usage documentation.

use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{parse2, parse_quote, punctuated::Punctuated, Result, Token};

mod types;

use self::types::*;

#[cfg(test)]
mod tests;

/// Print an error message.
#[proc_macro]
pub fn error(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	gum(item, Level::Error)
}

/// Print a warning level message.
#[proc_macro]
pub fn warn(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	gum(item, Level::Warn)
}

/// Print a warning or debug level message depending on their frequency
#[proc_macro]
pub fn warn_if_frequent(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let ArgsIfFrequent { freq, max_rate, rest } = parse2(item.into()).unwrap();

	let freq_expr = freq.expr;
	let max_rate_expr = max_rate.expr;
	let debug: proc_macro2::TokenStream = gum(rest.clone().into(), Level::Debug).into();
	let warn: proc_macro2::TokenStream = gum(rest.into(), Level::Warn).into();

	let stream = quote! {
		if #freq_expr .is_frequent(#max_rate_expr) {
			#warn
		} else {
			#debug
		}
	};

	stream.into()
}

/// Print a info level message.
#[proc_macro]
pub fn info(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	gum(item, Level::Info)
}

/// Print a debug level message.
#[proc_macro]
pub fn debug(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	gum(item, Level::Debug)
}

/// Print a trace level message.
#[proc_macro]
pub fn trace(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	gum(item, Level::Trace)
}

/// One-size-fits all internal implementation that produces the actual code.
pub(crate) fn gum(item: proc_macro::TokenStream, level: Level) -> proc_macro::TokenStream {
	let item: TokenStream = item.into();

	let res = expander::Expander::new("gum")
		.add_comment("Generated overseer code by `gum::warn!(..)`".to_owned())
		// `dry=true` until rust-analyzer can selectively disable features so it's
		// not all red squiggles. Originally: `!cfg!(feature = "expand")`
		// ISSUE: <https://github.com/rust-analyzer/rust-analyzer/issues/11777>
		.dry(true)
		.verbose(false)
		.fmt(expander::Edition::_2021)
		.maybe_write_to_out_dir(impl_gum2(item, level))
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	res.unwrap_or_else(|err| err.to_compile_error()).into()
}

/// Does the actual parsing and token generation based on `proc_macro2` types.
///
/// Required for unit tests.
pub(crate) fn impl_gum2(orig: TokenStream, level: Level) -> Result<TokenStream> {
	let args: Args = parse2(orig)?;

	let krate = support_crate();
	let span = Span::call_site();

	let Args { target, comma, mut values, fmt } = args;

	// find a value or alias called `candidate_hash`.
	let maybe_candidate_hash = values.iter_mut().find(|value| value.as_ident() == "candidate_hash");

	if let Some(kv) = maybe_candidate_hash {
		let (ident, rhs_expr, replace_with) = match kv {
			Value::Alias(alias) => {
				let ValueWithAliasIdent { alias, marker, expr, .. } = alias.clone();
				(
					alias.clone(),
					expr.to_token_stream(),
					Some(Value::Value(ValueWithFormatMarker {
						marker,
						ident: alias,
						dot: None,
						inner: Punctuated::new(),
					})),
				)
			},
			Value::Value(value) => (value.ident.clone(), value.ident.to_token_stream(), None),
		};

		// we generate a local value with the same alias name
		// so replace the expr with just a value
		if let Some(replace_with) = replace_with {
			let _old = std::mem::replace(kv, replace_with);
		};

		// Inject the addition `traceID = % trace_id` identifier
		// while maintaining trailing comma semantics.
		let had_trailing_comma = values.trailing_punct();
		if !had_trailing_comma {
			values.push_punct(Token![,](span));
		}

		values.push_value(parse_quote! {
			traceID = % trace_id
		});
		if had_trailing_comma {
			values.push_punct(Token![,](span));
		}

		Ok(quote! {
			if #krate :: enabled!(#target #comma #level) {
				use ::std::ops::Deref;

				// create a scoped let binding of something that `deref`s to
				// `Hash`.
				let value = #rhs_expr;
				let value = &value;
				let value: & #krate:: Hash = value.deref();
				// Do the `deref` to `Hash` and convert to a `TraceIdentifier`.
				let #ident: #krate:: Hash = * value;
				let trace_id = #krate:: hash_to_trace_identifier ( #ident );
				#krate :: event!(
					#target #comma #level, #values #fmt
				)
			}
		})
	} else {
		Ok(quote! {
				#krate :: event!(
					#target #comma #level, #values #fmt
				)
		})
	}
}

/// Extract the support crate path.
fn support_crate() -> TokenStream {
	let support_crate_name = if cfg!(test) {
		quote! {crate}
	} else {
		use proc_macro_crate::{crate_name, FoundCrate};
		let crate_name = crate_name("tracing-gum")
			.expect("Support crate `tracing-gum` is present in `Cargo.toml`. qed");
		match crate_name {
			FoundCrate::Itself => quote! {crate},
			FoundCrate::Name(name) => Ident::new(&name, Span::call_site()).to_token_stream(),
		}
	};
	support_crate_name
}
