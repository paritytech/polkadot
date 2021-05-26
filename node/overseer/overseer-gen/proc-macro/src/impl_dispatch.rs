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

use super::*;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Path, Result};

pub(crate) fn impl_dispatch(info: &OverseerInfo) -> Result<TokenStream> {
	let message_wrapper = &info.message_wrapper;

	let dispatchable = info
		.subsystems()
		.into_iter()
		.filter(|ssf| !ssf.no_dispatch)
		.map(|ssf| ssf.consumes.clone())
		.collect::<Vec<Path>>();

	let extern_event_ty = &info.extern_event_ty.clone();

	let ts = quote! {
		impl #message_wrapper {
			/// Generated dispatch iterator generator.
			pub fn dispatch_iter(event: #extern_event_ty) -> impl Iterator<Item=Self> + Send {
				let mut iter = None.into_iter();

				use ::std::convert::TryFrom;

				// creates pretty errors when the inner variant
				// does not impl `TryFrom< #extern_event_ty >`
				fn dispatchable_message_impls_try_from_extern_event<T: TryFrom< #extern_event_ty >>() {
				}

				#(
					dispatchable_message_impls_try_from_extern_event::< #dispatchable >();

					let mut iter = iter.chain(
						::std::iter::once(
							#dispatchable :: try_from( event ).ok().map(|event| {
								#message_wrapper :: #dispatchable ( event )
							})
						)
					);
				)*
				iter.filter_map(|x: Option<_>| x)
			}
		}
	};
	Ok(ts)
}
