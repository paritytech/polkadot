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

#![deny(unused_crate_dependencies)]

use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens, format_ident};
use syn::{parse2, spanned::Spanned, Result, punctuated::Punctuated, parse_quote, Token};

mod impl_builder;
mod impl_channels_out;
mod impl_dispatch;
mod impl_message_wrapper;
mod impl_overseer;
mod impl_subsystem;
mod parse;

use impl_builder::*;
use impl_channels_out::*;
use impl_dispatch::*;
use impl_message_wrapper::*;
use impl_overseer::*;
use impl_subsystem::*;
use parse::*;

#[cfg(test)]
mod tests;

#[proc_macro_attribute]
pub fn overlord(
	attr: proc_macro::TokenStream,
	item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
	let attr: TokenStream = attr.into();
	let item: TokenStream = item.into();
	impl_overseer_gen(attr, item)
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
	impl_subsystem_gen(attr, item)
		.unwrap_or_else(|err| err.to_compile_error())
		.into()
}

pub(crate) fn impl_overseer_gen(
	attr: TokenStream,
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let args: AttrArgs = parse2(attr)?;
	let message_wrapper = args.message_wrapper;

	let of: OverseerGuts = parse2(orig)?;

	let support_crate_name = if cfg!(test) {
		quote! {crate}
	} else {
		use proc_macro_crate::{crate_name, FoundCrate};
		let crate_name = crate_name("polkadot-overseer-gen")
			.expect("Support crate polkadot-overseer-gen is present in `Cargo.toml`. qed");
		match crate_name {
			FoundCrate::Itself => quote! {crate},
			FoundCrate::Name(name) => Ident::new(&name, Span::call_site()).to_token_stream(),
		}
	};
	let info = OverseerInfo {
		support_crate_name,
		subsystems: of.subsystems,
		baggage: of.baggage,
		overseer_name: of.name,
		message_wrapper,
		message_channel_capacity: args.message_channel_capacity,
		signal_channel_capacity: args.signal_channel_capacity,
		extern_event_ty: args.extern_event_ty,
		extern_signal_ty: args.extern_signal_ty,
		extern_error_ty: args.extern_error_ty,
		extern_network_ty: args.extern_network_ty,
		outgoing_ty: args.outgoing_ty,
	};

	let mut additive = impl_overseer_struct(&info);
	additive.extend(impl_builder(&info));

	additive.extend(impl_overseen_subsystem(&info));
	additive.extend(impl_channels_out_struct(&info));
	additive.extend(impl_subsystem(&info)?);

	additive.extend(impl_message_wrapper_enum(&info)?);
	additive.extend(impl_dispatch(&info));

	let ts = expander::Expander::new("overlord-expansion")
		.add_comment("Generated overseer code by `#[overlord(..)]`".to_owned())
		.dry(!cfg!(feature = "expand"))
		.verbose(true)
		.fmt(expander::Edition::_2021)
		.write_to_out_dir(additive)
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	Ok(ts)
}


pub(crate) fn impl_subsystem_gen(
	attr: TokenStream,
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	if !attr.is_empty() {
		return Err(
			syn::Error::new(
				attr.span(),
				"Doesn't take any arguments at this time"
			)
		);
	}
	let mut struktured_impl: syn::ItemImpl = parse2(orig)?;

	let support_crate = if cfg!(test) {
		quote! {crate}
	} else {
		use proc_macro_crate::{crate_name, FoundCrate};
		let crate_name = crate_name("polkadot-overseer-gen")
			.expect("Support crate polkadot-overseer-gen is present in `Cargo.toml`. qed");
		match crate_name {
			FoundCrate::Itself => quote! {crate},
			FoundCrate::Name(name) => Ident::new(&name, Span::call_site()).to_token_stream(),
		}
	};

	let me = match &*struktured_impl.self_ty {
		syn::Type::Path(ref path) => {
			dbg!(path).path.segments.last().expect("Must be ident. FIXME bail properly").ident.clone()
		},
		_ => {
			return Err(
				syn::Error::new(
					attr.span(),
					"Doesn't take any arguments at this time"
				)
			)
		}
	};
	let subsystem_ctx_trait = format_ident!("{}ContextTrait", me);
	let subsystem_sender_trait = format_ident!("{}SenderTrait", me);
	let extra_where_predicates: Punctuated<syn::WherePredicate, syn::Token![,]> = parse_quote!{
		Context: #subsystem_ctx_trait,
		Context: #support_crate::SubsystemContext,
		<Context as #subsystem_ctx_trait>::Sender: #subsystem_sender_trait,
		<Context as #support_crate::SubsystemContext>::Sender: #subsystem_sender_trait,
	};
	let error_ty = quote!{ Yikes }; // FIXME
	struktured_impl.trait_.replace(
		(
			None,
			parse_quote!{ #support_crate::Subsystem<Context, #error_ty>},
			syn::token::For::default(),
		)
	);
	let where_clause = struktured_impl.generics.make_where_clause();
	where_clause.predicates.extend(extra_where_predicates);


	let ts = expander::Expander::new(format!("subsystem-{}-expansion", subsystem_ctx_trait.to_string().to_lowercase()))
		.add_comment("Generated overseer code by `#[subsystem(..)]`".to_owned())
		.dry(!cfg!(feature = "expand"))
		.verbose(true)
		.fmt(expander::Edition::_2021)
		.write_to_out_dir(struktured_impl.into_token_stream())
		.expect("Expander does not fail due to IO in OUT_DIR. qed");

	Ok(ts)
}
