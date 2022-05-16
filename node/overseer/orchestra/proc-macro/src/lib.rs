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

use proc_macro2::{Ident, Span, TokenStream};
use syn::{parse_quote, spanned::Spanned, Path};

mod impl_builder;
mod impl_channels_out;
mod impl_message_wrapper;
mod impl_overseer;
mod impl_subsystem_ctx_sender;
mod overseer;
mod parse;
mod subsystem;

#[cfg(test)]
mod tests;

use impl_builder::*;
use impl_channels_out::*;
use impl_message_wrapper::*;
use impl_overseer::*;
use impl_subsystem_ctx_sender::*;
use parse::*;

use self::{overseer::*, subsystem::*};

/// Obtain the support crate `Path` as `TokenStream`.
pub(crate) fn support_crate() -> Result<Path, proc_macro_crate::Error> {
	Ok(if cfg!(test) {
		parse_quote! {crate}
	} else {
		use proc_macro_crate::{crate_name, FoundCrate};
		let crate_name = crate_name("orchestra")?;
		match crate_name {
			FoundCrate::Itself => parse_quote! {crate},
			FoundCrate::Name(name) => Ident::new(&name, Span::call_site()).into(),
		}
	})
}

#[proc_macro_attribute]
pub fn orchestra(
	attr: proc_macro::TokenStream,
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impl_orchestra_gen(attr, item)
		.unwrap_or_else(|err| err.to_compile_error())
		.into()
}

#[proc_macro_attribute]
pub fn subsystem(
	attr: proc_macro::TokenStream,
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impl_subsystem_context_trait_bounds(attr, item, MakeSubsystem::ImplSubsystemTrait)
		.unwrap_or_else(|err| err.to_compile_error())
		.into()
}

#[proc_macro_attribute]
pub fn contextbounds(
	attr: proc_macro::TokenStream,
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impl_subsystem_context_trait_bounds(attr, item, MakeSubsystem::AddContextTraitBounds)
		.unwrap_or_else(|err| err.to_compile_error())
		.into()
}
