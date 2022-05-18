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

use quote::quote;
use syn::{spanned::Spanned, Result};

use super::*;

/// Generates the wrapper type enum.
pub(crate) fn impl_message_wrapper_enum(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let consumes = info.any_message();
	let consumes_variant = info.variant_names();

	let outgoing = &info.outgoing_ty;

	let message_wrapper = &info.message_wrapper;

	let (outgoing_from_impl, outgoing_decl) = if let Some(outgoing) = outgoing {
		let outgoing_variant = outgoing.get_ident().ok_or_else(|| {
			syn::Error::new(
				outgoing.span(),
				"Missing identifier to use as enum variant for outgoing.",
			)
		})?;
		(
			quote! {
				impl ::std::convert::From< #outgoing > for #message_wrapper {
					fn from(message: #outgoing) -> Self {
						#message_wrapper :: #outgoing_variant ( message )
					}
				}
			},
			quote! {
				#outgoing_variant ( #outgoing ) ,
			},
		)
	} else {
		(TokenStream::new(), TokenStream::new())
	};

	let ts = quote! {
		/// Generated message type wrapper over all possible messages
		/// used by any subsystem.
		#[allow(missing_docs)]
		#[derive(Debug)]
		pub enum #message_wrapper {
			#(
				#consumes_variant ( #consumes ),
			)*
			#outgoing_decl
			// dummy message type
			Empty,
		}

		impl ::std::convert::From< () > for #message_wrapper {
			fn from(_: ()) -> Self {
				#message_wrapper :: Empty
			}
		}

		#(
			impl ::std::convert::From< #consumes > for #message_wrapper {
				fn from(message: #consumes) -> Self {
					#message_wrapper :: #consumes_variant ( message )
				}
			}
		)*

		#outgoing_from_impl
	};

	Ok(ts)
}
