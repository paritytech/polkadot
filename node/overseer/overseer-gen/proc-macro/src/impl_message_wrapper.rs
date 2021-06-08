use quote::quote;
use syn::Result;

use super::*;

/// Generates the wrapper type enum.
pub(crate) fn impl_message_wrapper_enum(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let consumes = info.consumes();

	let message_wrapper = &info.message_wrapper;

	let ts = quote! {
		/// Generated message type wrapper
		#[allow(missing_docs)]
		#[derive(Debug)]
		pub enum #message_wrapper {
			#(
				#consumes ( #consumes ),
			)*
		}

		#(
		impl ::std::convert::From< #consumes > for #message_wrapper {
			fn from(message: #consumes) -> Self {
				#message_wrapper :: #consumes ( message )
			}
		}
		)*
	};

	Ok(ts)
}
