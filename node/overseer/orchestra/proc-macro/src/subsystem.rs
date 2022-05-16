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

//! Generates the bounds for a particular subsystem `Context` and associate `type Sender`.
//!
//!
//! ## Implement `trait Subsystem<Context, Error>` via `subsystem`
//!
//! ```ignore
//! # use orchestra_proc_macro::subsystem;
//! # mod somewhere {
//! # use orchestra_proc_macro::orchestra;
//! # pub use orchestra::*;
//! #
//! # #[derive(Debug, thiserror::Error)]
//! # #[error("Yikes!")]
//! # pub struct Yikes;
//! # impl From<OverseerError> for Yikes {
//! #   fn from(_: OverseerError) -> Yikes { Yikes }
//! # }
//! # impl From<mpsc::SendError> for Yikes {
//! #   fn from(_: mpsc::SendError) -> Yikes { Yikes }
//! # }
//! #
//! # #[derive(Debug)]
//! # pub struct Eve;
//! #
//! # #[derive(Debug, Clone)]
//! # pub struct Sig;
//! #
//! # #[derive(Debug, Clone, Copy)]
//! # pub struct A;
//! # #[derive(Debug, Clone, Copy)]
//! # pub struct B;
//! #
//! # #[orchestra(signal=Sig, gen=AllOfThem, event=Eve, error=Yikes)]
//! # pub struct Wonderland {
//! # 	#[subsystem(A, sends: [B])]
//! # 	foo: Foo,
//! # 	#[subsystem(B, sends: [A])]
//! # 	bar: Bar,
//! # }
//! # }
//! # use somewhere::{Yikes, SpawnedSubsystem};
//! #
//! # struct FooSubsystem;
//! #
//! #[subsystem(Foo, error = Yikes, prefix = somewhere)]
//! impl<Context> FooSubsystem {
//!    fn start(self, context: Context) -> SpawnedSubsystem<Yikes> {
//!	       // ..
//!        # let _ = context;
//!        # unimplemented!()
//!    }
//! }
//! ```
//!
//! expands to
//!
//! ```ignore
//! # use orchestra_proc_macro::subsystem;
//! # mod somewhere {
//! # use orchestra_proc_macro::orchestra;
//! # pub use orchestra::*;
//! #
//! # #[derive(Debug, thiserror::Error)]
//! # #[error("Yikes!")]
//! # pub struct Yikes;
//! # impl From<OverseerError> for Yikes {
//! #   fn from(_: OverseerError) -> Yikes { Yikes }
//! # }
//! # impl From<mpsc::SendError> for Yikes {
//! #   fn from(_: mpsc::SendError) -> Yikes { Yikes }
//! # }
//! #
//! # #[derive(Debug)]
//! # pub struct Eve;
//! #
//! # #[derive(Debug, Clone)]
//! # pub struct Sig;
//! #
//! # #[derive(Debug, Clone, Copy)]
//! # pub struct A;
//! # #[derive(Debug, Clone, Copy)]
//! # pub struct B;
//! #
//! # #[orchestra(signal=Sig, gen=AllOfThem, event=Eve, error=Yikes)]
//! # pub struct Wonderland {
//! # 	#[subsystem(A, sends: [B])]
//! # 	foo: Foo,
//! # 	#[subsystem(B, sends: [A])]
//! # 	bar: Bar,
//! # }
//! # }
//! # use somewhere::{Yikes, SpawnedSubsystem};
//! # use orchestra as support_crate;
//! #
//! # struct FooSubsystem;
//! #
//! impl<Context> support_crate::Subsystem<Context, Yikes> for FooSubsystem
//! where
//! 	Context: somewhere::FooContextTrait,
//!     Context: support_crate::SubsystemContext,
//!		<Context as somewhere::FooContextTrait>::Sender: somewhere::FooSenderTrait,
//!		<Context as support_crate::SubsystemContext>::Sender: somewhere::FooSenderTrait,
//! {
//!       fn start(self, context: Context) -> SpawnedSubsystem<Yikes> {
//!        // ..
//!        # let _ = context;
//!        # unimplemented!()
//!       }
//! }
//! ```
//!
//! where `support_crate` is either equivalent to `somewhere` or derived from the cargo manifest.
//!
//!
//! ## Add additional trait bounds for a generic `Context` via `contextbounds`
//!
//! ### To an `ImplItem`
//!
//! ```ignore
//! #[contextbounds(Foo, prefix = somewhere)]
//! impl<Context> X {
//! ..
//! }
//! ```
//!
//! expands to
//!
//! ```ignore
//! impl<Context> X
//! where
//! 	Context: somewhere::FooSubsystemTrait,
//!     Context: support_crate::SubsystemContext,
//!		<Context as somewhere::FooContextTrait>::Sender: somewhere::FooSenderTrait,
//!		<Context as support_crate::SubsystemContext>::Sender: somewhere::FooSenderTrait,
//! {
//! }
//! ```
//!
//! ### To a free standing `Fn` (not a method, that's covered by the above)
//!
//! ```ignore
//! #[contextbounds(Foo, prefix = somewhere)]
//! fn do_smth<Context>(context: &mut Context) {
//! ..
//! }
//! ```
//!
//! expands to
//!
//! ```ignore
//! fn do_smth<Context>(context: &mut Context)
//! where
//! 	Context: somewhere::FooSubsystemTrait,
//!     Context: support_crate::SubsystemContext,
//!		<Context as somewhere::FooContextTrait>::Sender: somewhere::FooSenderTrait,
//!		<Context as support_crate::SubsystemContext>::Sender: somewhere::FooSenderTrait,
//! {
//! }
//! ```
use proc_macro2::TokenStream;
use quote::{format_ident, ToTokens};
use syn::{parse2, parse_quote, punctuated::Punctuated, Result};

use super::{parse::*, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MakeSubsystem {
	/// Implements `trait Subsystem` and apply the trait bounds to the `Context` generic.
	///
	/// Relevant to `impl Item` only.
	ImplSubsystemTrait,
	/// Only apply the trait bounds to the context.
	AddContextTraitBounds,
}

pub(crate) fn impl_subsystem_context_trait_bounds(
	attr: TokenStream,
	orig: TokenStream,
	make_subsystem: MakeSubsystem,
) -> Result<proc_macro2::TokenStream> {
	let args = parse2::<SubsystemAttrArgs>(attr.clone())?;
	let span = args.span();
	let SubsystemAttrArgs { error_path, subsystem_ident, trait_prefix_path, .. } = args;

	let mut item = parse2::<syn::Item>(orig)?;

	// always prefer the direct usage, if it's not there, let's see if there is
	// a `prefix=*` provided. Either is ok.

	// Technically this is two different things:
	// The place where the `#[orchestra]` is annotated is where all `trait *SenderTrait` and
	// `trait *ContextTrait` types exist.
	// The other usage is the true support crate `orchestra`, where the static ones
	// are declared.
	// Right now, if the `support_crate` is not included, it falls back silently to the `trait_prefix_path`.
	let support_crate = support_crate()
		.or_else(|_e| {
			trait_prefix_path.clone().ok_or_else(|| {
				syn::Error::new(attr.span(), "Couldn't find `orchestra` in manifest, but also missing a `prefix=` to help trait bound resolution")
			})
		})?;

	let trait_prefix_path = trait_prefix_path.unwrap_or_else(|| parse_quote! { self });
	if trait_prefix_path.segments.trailing_punct() {
		return Err(syn::Error::new(trait_prefix_path.span(), "Must not end with `::`"))
	}

	let subsystem_ctx_trait = format_ident!("{}ContextTrait", subsystem_ident);
	let subsystem_sender_trait = format_ident!("{}SenderTrait", subsystem_ident);

	let extra_where_predicates: Punctuated<syn::WherePredicate, syn::Token![,]> = parse_quote! {
		Context: #trait_prefix_path::#subsystem_ctx_trait,
		Context: #support_crate::SubsystemContext,
		<Context as #trait_prefix_path::#subsystem_ctx_trait>::Sender: #trait_prefix_path::#subsystem_sender_trait,
		<Context as #support_crate::SubsystemContext>::Sender: #trait_prefix_path::#subsystem_sender_trait,
	};

	let apply_ctx_bound_if_present = move |generics: &mut syn::Generics| -> bool {
		if generics
			.params
			.iter()
			.find(|generic| match generic {
				syn::GenericParam::Type(ty) if ty.ident == "Context" => true,
				_ => false,
			})
			.is_some()
		{
			let where_clause = generics.make_where_clause();
			where_clause.predicates.extend(extra_where_predicates.clone());
			true
		} else {
			false
		}
	};

	match item {
		syn::Item::Impl(ref mut struktured_impl) => {
			if make_subsystem == MakeSubsystem::ImplSubsystemTrait {
				let error_path = error_path.ok_or_else(|| {
					syn::Error::new(
						span,
						"Must annotate the identical overseer error type via `error=..`.",
					)
				})?;
				// Only replace the subsystem trait if it's desired.
				struktured_impl.trait_.replace((
					None,
					parse_quote! {
						#support_crate::Subsystem<Context, #error_path>
					},
					syn::token::For::default(),
				));
			}

			apply_ctx_bound_if_present(&mut struktured_impl.generics);
			for item in struktured_impl.items.iter_mut() {
				match item {
					syn::ImplItem::Method(method) => {
						apply_ctx_bound_if_present(&mut method.sig.generics);
					},
					_others => {
						// don't error, just nop
					},
				}
			}
		},
		syn::Item::Fn(ref mut struktured_fn) => {
			if make_subsystem == MakeSubsystem::ImplSubsystemTrait {
				return Err(syn::Error::new(struktured_fn.span(), "Cannot make a free function a subsystem, did you mean to apply `contextbound` instead?"))
			}
			apply_ctx_bound_if_present(&mut struktured_fn.sig.generics);
		},
		other =>
			return Err(syn::Error::new(
				other.span(),
				"Macro can only be annotated on functions or struct implementations",
			)),
	};

	Ok(item.to_token_stream())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn is_path() {
		let _p: Path = parse_quote! { self };
		let _p: Path = parse_quote! { crate };
		let _p: Path = parse_quote! { ::foo };
		let _p: Path = parse_quote! { bar };
	}
}
