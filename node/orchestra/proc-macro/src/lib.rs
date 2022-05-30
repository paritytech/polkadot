// Copyright (C) 2021 Parity Technologies (UK) Ltd.
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

use proc_macro2::{Ident, Span, TokenStream};
use syn::{parse_quote, spanned::Spanned, Path};

mod graph;
mod impl_builder;
mod impl_channels_out;
mod impl_message_wrapper;
mod impl_orchestra;
mod impl_subsystem_ctx_sender;
mod orchestra;
mod parse;
mod subsystem;

#[cfg(test)]
mod tests;

use impl_builder::*;
use impl_channels_out::*;
use impl_message_wrapper::*;
use impl_orchestra::*;
use impl_subsystem_ctx_sender::*;
use parse::*;

use self::{orchestra::*, subsystem::*};

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
