// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Procedural macros used in XCM.

use proc_macro::TokenStream;

mod v2;
mod v3;
mod weight_info;

#[proc_macro]
pub fn impl_conversion_functions_for_multilocation_v2(input: TokenStream) -> TokenStream {
	v2::multilocation::generate_conversion_functions(input)
		.unwrap_or_else(syn::Error::into_compile_error)
		.into()
}

#[proc_macro_derive(XcmWeightInfoTrait)]
pub fn derive_xcm_weight_info(item: TokenStream) -> TokenStream {
	weight_info::derive(item)
}

#[proc_macro]
pub fn impl_conversion_functions_for_multilocation_v3(input: TokenStream) -> TokenStream {
	v3::multilocation::generate_conversion_functions(input)
		.unwrap_or_else(syn::Error::into_compile_error)
		.into()
}

#[proc_macro]
pub fn impl_conversion_functions_for_junctions_v3(input: TokenStream) -> TokenStream {
	v3::junctions::generate_conversion_functions(input)
		.unwrap_or_else(syn::Error::into_compile_error)
		.into()
}
