use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::HashSet;

use quote::ToTokens;
use syn::AttrStyle;
use syn::Field;
use syn::FieldsNamed;
use syn::Result;
use syn::Variant;

use super::*;

pub(crate) fn include_static_rs() -> Result<proc_macro2::TokenStream> {
	use std::str::FromStr;

	let s = include_str!("./inc/static.rs");
	let ts = proc_macro2::TokenStream::from_str(s)?;
	Ok(ts)
}
