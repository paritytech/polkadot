// Copyright 2020 Parity Technologies (UK) Ltd.
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
use syn::{Result, Token};

pub fn generate_v1_conversion_functions(input: proc_macro::TokenStream) -> Result<TokenStream> {
	if !input.is_empty() {
		return Err(syn::Error::new(Span::call_site(), "No arguments expected"))
	}

	let from_tuples = generate_conversion_from_tuples();
	let from_v0 = generate_conversion_from_v0();

	Ok(quote! {
		#from_tuples
		#from_v0
	})
}

fn generate_conversion_from_tuples() -> TokenStream {
	let mut from_tuples = TokenStream::new();

	for num_junctions in 0..8usize {
		let junctions = (0..=num_junctions).map(|_| format_ident!("Junction")).collect::<Vec<_>>();
		let idents = (0..=num_junctions).map(|i| format_ident!("j{}", i)).collect::<Vec<_>>();
		let variant = &format_ident!("X{}", num_junctions + 1);
		let array_size = num_junctions + 1;

		let from_tuple = quote! {
			impl From<( #(#junctions,)* )> for MultiLocation {
				fn from( ( #(#idents,)* ): ( #(#junctions,)* ) ) -> Self {
					MultiLocation { parents: 0, interior: Junctions::#variant( #(#idents),* ) }
				}
			}

			impl From<(u8, #(#junctions),*)> for MultiLocation {
				fn from( ( parents, #(#idents),* ): (u8, #(#junctions),* ) ) -> Self {
					MultiLocation { parents, interior: Junctions::#variant( #(#idents),* ) }
				}
			}

			impl From<(Ancestor, #(#junctions),*)> for MultiLocation {
				fn from( ( Ancestor(parents), #(#idents),* ): (Ancestor, #(#junctions),* ) ) -> Self {
					MultiLocation { parents, interior: Junctions::#variant( #(#idents),* ) }
				}
			}

			impl From<[Junction; #array_size]> for MultiLocation {
				fn from(j: [Junction; #array_size]) -> Self {
					let [#(#idents),*] = j;
					MultiLocation { parents: 0, interior: Junctions::#variant( #(#idents),* ) }
				}
			}
		};

		// Support up to 8 Parents in a tuple
		for cur_parents in 1..=8u8 {
			let parents = (0..cur_parents).map(|_| format_ident!("Parent")).collect::<Vec<_>>();
			let underscores =
				(0..cur_parents).map(|_| Token![_](Span::call_site())).collect::<Vec<_>>();

			from_tuples.extend(quote! {
				impl From<( #(#parents,)* #(#junctions),* )> for MultiLocation {
					fn from( (#(#underscores,)* #(#idents),*): ( #(#parents,)* #(#junctions),* ) ) -> Self {
						MultiLocation { parents: #cur_parents, interior: Junctions::#variant( #(#idents),* ) }
					}
				}
			});
		}

		from_tuples.extend(from_tuple);
	}

	// Support up to 8 Parents in a tuple
	for cur_parents in 1..=8u8 {
		let parents = (0..cur_parents).map(|_| format_ident!("Parent")).collect::<Vec<_>>();
		let underscores =
			(0..cur_parents).map(|_| Token![_](Span::call_site())).collect::<Vec<_>>();

		from_tuples.extend(quote! {
			impl From<( #(#parents,)* Junctions )> for MultiLocation {
				fn from( (#(#underscores,)* junctions): ( #(#parents,)* Junctions ) ) -> Self {
					MultiLocation { parents: #cur_parents, interior: junctions }
				}
			}
		});
	}

	quote! {
		impl From<Junctions> for MultiLocation {
			fn from(junctions: Junctions) -> Self {
				MultiLocation { parents: 0, interior: junctions }
			}
		}

		impl From<(u8, Junctions)> for MultiLocation {
			fn from((parents, interior): (u8, Junctions)) -> Self {
				MultiLocation { parents, interior }
			}
		}

		impl From<(Ancestor, Junctions)> for MultiLocation {
			fn from((Ancestor(parents), interior): (Ancestor, Junctions)) -> Self {
				MultiLocation { parents, interior }
			}
		}

		impl From<()> for MultiLocation {
			fn from(_: ()) -> Self {
				MultiLocation { parents: 0, interior: Junctions::Here }
			}
		}

		impl From<(u8,)> for MultiLocation {
			fn from((parents,): (u8,)) -> Self {
				MultiLocation { parents, interior: Junctions::Here }
			}
		}

		impl From<Junction> for MultiLocation {
			fn from(x: Junction) -> Self {
				MultiLocation { parents: 0, interior: Junctions::X1(x) }
			}
		}

		impl From<[Junction; 0]> for MultiLocation {
			fn from(_: [Junction; 0]) -> Self {
				MultiLocation { parents: 0, interior: Junctions::Here }
			}
		}

		#from_tuples
	}
}

fn generate_conversion_from_v0() -> TokenStream {
	let mut match_variants = TokenStream::new();

	for cur_num in 0..8u8 {
		let mut intermediate_match_arms = TokenStream::new();
		let num_ancestors = cur_num + 1;
		let variant = format_ident!("X{}", num_ancestors);
		let idents = (0..=cur_num).map(|i| format_ident!("j{}", i)).collect::<Vec<_>>();

		for parent_count in (1..num_ancestors).rev() {
			let parent_idents =
				(0..parent_count).map(|j| format_ident!("j{}", j)).collect::<Vec<_>>();
			let junction_idents = (parent_count..num_ancestors)
				.map(|j| format_ident!("j{}", j))
				.collect::<Vec<_>>();
			let junction_variant = format_ident!("X{}", num_ancestors - parent_count);

			intermediate_match_arms.extend(quote! {
				crate::v0::MultiLocation::#variant( #(#idents),* )
					if #( #parent_idents.is_parent() )&&* =>
					Ok(MultiLocation {
						parents: #parent_count,
						interior: #junction_variant( #( #junction_idents.try_into()? ),* ),
					}),
			});
		}

		match_variants.extend(quote! {
			crate::v0::MultiLocation::#variant( #(#idents),* )
				if #( #idents.is_parent() )&&* =>
				Ok(MultiLocation::ancestor(#num_ancestors)),
			#intermediate_match_arms
			crate::v0::MultiLocation::#variant( #(#idents),* ) =>
				Ok( #variant( #( #idents.try_into()? ),* ).into() ),
		});
	}

	quote! {
		impl TryFrom<crate::v0::MultiLocation> for MultiLocation {
			type Error = ();
			fn try_from(v0: crate::v0::MultiLocation) -> core::result::Result<Self, ()> {
				use Junctions::*;
				match v0 {
					crate::v0::MultiLocation::Null => Ok(Here.into()),
					#match_variants
				}
			}
		}
	}
}
