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
use quote::{format_ident, quote, ToTokens};
use syn::{parse2, parse_quote, punctuated::Punctuated, spanned::Spanned, Result};

use super::{parse::*, *};

pub(crate) fn impl_subsystem_gen(
	attr: TokenStream,
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let SubsystemAttrArgs { extern_error_ty: error_ty, subsystem_ident, .. } =
		parse2::<SubsystemAttrArgs>(attr.clone())?;

	let mut struktured_impl: syn::ItemImpl = parse2(orig)?;

	let support_crate = support_crate();

	let me = match &*struktured_impl.self_ty {
		syn::Type::Path(ref path) => path
			.path
			.segments
			.last()
			.ok_or_else(|| {
				syn::Error::new(
					path.path.span(),
					"Missing an identifier at the end of the path provided",
				)
			})?
			.ident
			.clone(),
		_ => return Err(syn::Error::new(attr.span(), "Doesn't take any arguments at this time")),
	};
	let subsystem_ctx_trait = format_ident!("{}ContextTrait", subsystem_ident);
	let subsystem_sender_trait = format_ident!("{}SenderTrait", subsystem_ident);
	let extra_where_predicates: Punctuated<syn::WherePredicate, syn::Token![,]> = parse_quote! {
		Context: #subsystem_ctx_trait,
		Context: #support_crate::SubsystemContext,
		<Context as #subsystem_ctx_trait>::Sender: #subsystem_sender_trait,
		<Context as #support_crate::SubsystemContext>::Sender: #subsystem_sender_trait,
	};
	struktured_impl.trait_.replace((
		None,
		parse_quote! {
			#support_crate::Subsystem<Context, #error_ty>
		},
		syn::token::For::default(),
	));
	let where_clause = struktured_impl.generics.make_where_clause();
	where_clause.predicates.extend(extra_where_predicates);

	let ts =
		expander::Expander::new(format!("subsystem-{}-expansion", me.to_string().to_lowercase()))
			.add_comment("Generated overseer code by `#[subsystem(..)]`".to_owned())
			.dry(!cfg!(feature = "expand"))
			.verbose(true)
			.fmt(expander::Edition::_2021)
			.write_to_out_dir(struktured_impl.into_token_stream())
			.expect("Expander does not fail due to IO in OUT_DIR. qed");

	Ok(ts)
}
