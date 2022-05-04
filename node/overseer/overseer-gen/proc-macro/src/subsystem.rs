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

use proc_macro2::TokenStream;
use quote::{format_ident, ToTokens};
use syn::{parse2, parse_quote, punctuated::Punctuated, Result};

use super::{parse::*, *};

pub(crate) fn impl_subsystem_gen(
	attr: TokenStream,
	orig: TokenStream,
) -> Result<proc_macro2::TokenStream> {
	let args = parse2::<SubsystemAttrArgs>(attr.clone())?;
	let _span = args.span();
	let SubsystemAttrArgs {
		error_ty,
		 subsystem_ident,
		 trait_prefix_path
		 , .. } =
		args;

	let mut struktured_impl: syn::ItemImpl = parse2(orig)?;

	// always prefer the direct usage, if it's not there, let's see if there is
	// a `prefix=*` provided. Either is ok.
	// TODO: technically this is two different things:
	// The place where the `#[overlord]` is annotated is where all `trait *SenderTrait` and
	// `trait *ContextTrait` types exist.
	// The other usage is the true support crate `polkadot-overseer-gen`, where the static ones
	// are declared.
	// Right now, if the `support_crate` is not included, it falls back silently to the `trait_prefix_path`.
	let support_crate = support_crate()
		.or_else(|_e| {
			trait_prefix_path.clone().ok_or_else(|| {
				syn::Error::new(attr.span(), "Couldn't find `polkadot-overseer-gen` in manifest, but also missing a `prefix=` to help trait bound resolution")
			})
		})?;

	let trait_prefix_path = trait_prefix_path.unwrap_or_else(|| parse_quote!{ self });
	if trait_prefix_path.segments.trailing_punct() {
		return Err(syn::Error::new(trait_prefix_path.span(), "Must not end with `::`"))
	}
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
		Context: #trait_prefix_path::#subsystem_ctx_trait,
		Context: #support_crate::SubsystemContext,
		<Context as #trait_prefix_path::#subsystem_ctx_trait>::Sender: #trait_prefix_path::#subsystem_sender_trait,
		<Context as #support_crate::SubsystemContext>::Sender: #trait_prefix_path::#subsystem_sender_trait,
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


#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn is_path() {
		let _p: Path = parse_quote!{ self };
		let _p: Path = parse_quote!{ crate };
		let _p: Path = parse_quote!{ ::foo };
		let _p: Path = parse_quote!{ bar };
	}
}
