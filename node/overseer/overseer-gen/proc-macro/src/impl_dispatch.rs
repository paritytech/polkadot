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
use proc_macro2::{TokenStream, Ident};
use quote::quote;
use syn::Path;

pub(crate) fn impl_dispatch(info: &OverseerInfo) -> TokenStream {
	let message_wrapper = &info.message_wrapper;

	let dispatchable_variant = info
		.subsystems()
		.into_iter()
		.filter(|ssf| !ssf.no_dispatch)
		.filter(|ssf| !ssf.wip)
		.map(|ssf| ssf.generic.clone())
		.collect::<Vec<Ident>>();

	let dispatchable_message = info
		.subsystems()
		.into_iter()
		.filter(|ssf| !ssf.no_dispatch)
		.filter(|ssf| !ssf.wip)
		.map(|ssf| ssf.consumes.clone())
		.collect::<Vec<Path>>();

	let mut ts = TokenStream::new();
	if let Some(extern_network_ty) = &info.extern_network_ty.clone() {
		ts.extend(quote! {
			impl #message_wrapper {
				/// Generated dispatch iterator generator.
				pub fn dispatch_iter(extern_msg: #extern_network_ty) -> impl Iterator<Item=Self> + Send {
					::std::array::IntoIter::new([
					#(
						extern_msg
							// focuses on a `NetworkBridgeEvent< protocol_v1::* >`
							// TODO do not require this to be hardcoded, either externalize or ...
							// https://github.com/paritytech/polkadot/issues/3427
							.focus()
							.ok()
							.map(|event| {
								#message_wrapper :: #dispatchable_variant (
									// the inner type of the enum variant
									#dispatchable_message :: from( event )
								)
							}),
					)*
					])
					.into_iter()
					.filter_map(|x: Option<_>| x)
				}
			}
		});
	}
	ts
}
