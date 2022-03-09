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
use syn::{parse2, Result};

#[cfg(test)]
mod tests;

#[proc_macro]
pub fn gum(
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let item: TokenStream = item.into();
	impl_gum(attr, item)
		.unwrap_or_else(|err| err.to_compile_error())
		.into()
}

pub(crate) fn impl_gum(
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let of: OverseerGuts = parse2(orig)?;


	let ts = expander::Expander::new("tracing-gum")
		.add_comment("Generated overseer code by `gum!(..)`".to_owned())
		.dry(!cfg!(feature = "expand"))
		.verbose(false)
		.fmt(expander::Edition::_2021)
		.write_to_out_dir(additive)
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	Ok(ts)
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
