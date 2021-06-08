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

	let mut ts = TokenStream::new();
	if let Some(extern_network_ty) = &info.extern_network_ty.clone() {
		ts.extend(quote! {
			impl #message_wrapper {
				/// Generated dispatch iterator generator.
				pub fn dispatch_iter(extern_msg: #extern_network_ty) -> impl Iterator<Item=Self> + Send {
					None
						.into_iter()

					#(
						.chain(
							::std::iter::once(
								extern_msg
									// focuses on a `NetworkBridgeEvent< protocol_v1::* >`
									.focus()
									.ok()
									.map(|event: NetworkBridgeEvent< _ >| {
										#message_wrapper :: #dispatchable (
											// the inner type of the enum variant
											#dispatchable :: from( event )
										)
									})
							)
						)
					)*

					.filter_map(|x: Option<_>| x)
				}
			}
		});
	}
	Ok(ts)
}
