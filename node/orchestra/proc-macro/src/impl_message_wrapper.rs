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

use quote::quote;
use syn::{spanned::Spanned, Result};

use super::*;

/// Generates the wrapper type enum.
pub(crate) fn impl_message_wrapper_enum(info: &OrchestraInfo) -> Result<proc_macro2::TokenStream> {
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
