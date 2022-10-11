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

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};

pub fn generate_conversion_functions(input: proc_macro::TokenStream) -> syn::Result<TokenStream> {
	if !input.is_empty() {
		return Err(syn::Error::new(Span::call_site(), "No arguments expected"))
	}

	let from_tuples = generate_conversion_from_tuples();
	let from_v1 = generate_conversion_from_v1();

	Ok(quote! {
		#from_tuples
		#from_v1
	})
}

fn generate_conversion_from_tuples() -> TokenStream {
	let from_tuples = (0..8usize)
		.map(|num_junctions| {
			let junctions =
				(0..=num_junctions).map(|_| format_ident!("Junction")).collect::<Vec<_>>();
			let idents = (0..=num_junctions).map(|i| format_ident!("j{}", i)).collect::<Vec<_>>();
			let variant = &format_ident!("X{}", num_junctions + 1);
			let array_size = num_junctions + 1;

			quote! {
				impl From<( #(#junctions,)* )> for MultiLocation {
					fn from( ( #(#idents,)* ): ( #(#junctions,)* ) ) -> Self {
						MultiLocation::#variant( #(#idents),* )
					}
				}

				impl From<[Junction; #array_size]> for MultiLocation {
					fn from(j: [Junction; #array_size]) -> Self {
						let [#(#idents),*] = j;
						MultiLocation::#variant( #(#idents),* )
					}
				}
			}
		})
		.collect::<TokenStream>();

	quote! {
		impl From<()> for MultiLocation {
			fn from(_: ()) -> Self {
				MultiLocation::Null
			}
		}

		impl From<Junction> for MultiLocation {
			fn from(x: Junction) -> Self {
				MultiLocation::X1(x)
			}
		}

		impl From<[Junction; 0]> for MultiLocation {
			fn from(_: [Junction; 0]) -> Self {
				MultiLocation::Null
			}
		}

		#from_tuples
	}
}

fn generate_conversion_from_v1() -> TokenStream {
	let match_variants = (0..8u8)
		.map(|cur_num| {
			let variant = format_ident!("X{}", cur_num + 1);
			let idents = (1..=cur_num).map(|i| format_ident!("j{}", i)).collect::<Vec<_>>();

			quote! {
				crate::v1::Junctions::#variant( j0 #(, #idents)* ) => res
					.pushed_with(Junction::from(j0))
					#( .and_then(|res| res.pushed_with(Junction::from(#idents))) )*
					.map_err(|_| ()),
			}
		})
		.collect::<TokenStream>();

	quote! {
		impl TryFrom<crate::v1::MultiLocation> for MultiLocation {
			type Error = ();
			fn try_from(v1: crate::v1::MultiLocation) -> core::result::Result<Self, ()> {
				let mut res = MultiLocation::Null;

				for _ in 0..v1.parents {
					res.push(Junction::Parent)?;
				}

				match v1.interior {
					crate::v1::Junctions::Here => Ok(res),
					#match_variants
				}
			}
		}
	}
}
