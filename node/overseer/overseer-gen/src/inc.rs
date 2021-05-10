







use syn::Result;

pub(crate) fn include_static_rs() -> Result<proc_macro2::TokenStream> {
	use std::str::FromStr;

	let s = include_str!("./inc/static.rs");
	let ts = proc_macro2::TokenStream::from_str(s)?;
	Ok(ts)
}
