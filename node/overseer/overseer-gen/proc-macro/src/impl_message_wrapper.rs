use quote::quote;
use syn::Result;

use super::*;

/// Generates the wrapper type enum.
pub(crate) fn impl_message_wrapper_enum(
	info: &OverseerInfo,
) -> Result<proc_macro2::TokenStream> {
	let consumes = info.consumes();

    let message_wrapper = &info.message_wrapper;

	let msg = "Generated message type wrapper";
	let ts = quote! {
		#[doc = #msg]
		#[derive(Debug, Clone)]
		enum #message_wrapper {
			#(
				#consumes ( #consumes ),
			)*
		}

		#(
		impl ::std::convert::From<#consumes> for #message_wrapper {
			fn from(src: #consumes) -> Self {
				#message_wrapper :: #consumes ( src )
			}
		}
		)*
	};

	Ok(ts)
}
