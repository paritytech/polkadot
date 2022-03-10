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

#![deny(unused_crate_dependencies)]

use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{Token, parse2, Result, punctuated::Punctuated, parse_quote};

mod types;

use self::types::*;

#[cfg(test)]
mod tests;

#[proc_macro]
pub fn gum(
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let item: TokenStream = item.into();

	let res = expander::Expander::new("tracing-gum")
		.add_comment("Generated overseer code by `gum!(..)`".to_owned())
		.dry(!cfg!(feature = "expand"))
		.verbose(false)
		.fmt(expander::Edition::_2021)
		.maybe_write_to_out_dir(impl_gum2(item))
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	res.unwrap_or_else(|err| err.to_compile_error())
		.into()
}

pub(crate) fn impl_gum2(
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let args: Args = parse2(orig)?;

	let ts = impl_gum2_from_args(args)?;

	Ok(ts)
}

fn impl_gum2_from_args(mut args: Args) -> Result<TokenStream> {
	let krate = support_crate();
	// TODO handle `candidate_hash` in a special way
	let span = Span::call_site();
	let maybe_candidate_hash = args.values.iter().find(|value| {
		let ident = value.as_ident();
		ident == &Ident::new("candidate_hash", ident.span())
	});
	if let Some(_candidate_hash) = maybe_candidate_hash {
		let had_trailing_comma = args.values.trailing_punct();
		if !had_trailing_comma {
			args.values.push_punct(Token![,](span));
		}
		// append the generaed `traceID` cross reference
		// XXX FIXME `candidate_hash = $expr` needs a let binding
		// that's guarded by something else
		args.values.push_value(parse_quote!{
			traceID = #krate :: hash_to_trace_identifier (candidate_hash)
		});
		if had_trailing_comma {
			args.values.push_punct(Token![,](span));
		}
	}

	Ok(quote!{
		#krate :: event!(
			#args
		)
	})
}

fn support_crate() -> TokenStream {
	let support_crate_name = if cfg!(test) {
		quote! {crate}
	} else {
		use proc_macro_crate::{crate_name, FoundCrate};
		let crate_name = crate_name("polkadot-node-tracing-gum")
			.expect("Support crate `polkadot-node-tracing-gum` is present in `Cargo.toml`. qed");
		match crate_name {
			FoundCrate::Itself => quote! {crate},
			FoundCrate::Name(name) => Ident::new(&name, Span::call_site()).to_token_stream(),
		}
	};
	support_crate_name
}
