use proc_macro2::{Ident, TokenStream, Span};
use quote::quote;
use syn::ItemEnum;

fn xxx(item: &mut ItemEnum) -> TokenStream {
	for varaint in item.variants.iter() {
		while let Some(idx) = item.attrs.iter().find_map(|attr| if attr.path.is_ident(Ident::new("fatality", Span::call_site()))) {
			name.attrs.swap_remove(ith)
		}
	}

	quote! {
		item

	}
}



#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn basic() {

		ts = quote!{
			r###"
#[fatality]
enum Kaboom {
	#[fatal, error(transparent)]
	A(X),
	#[error(transparent)]
	B(Y),
}
"###
		};
		let ts = all(ts).to_string();
		assert_eq!(r###"
#[::thiserror::Error]
enum BoringKaboom {
	#[error(transparent)]
	B(Y),
}

#[::thiserror::Error]
enum FatalKaboom {
	#[error(transparent)]
	A(X),
}

#[::thiserror::Error]
enum Kaboom {
	#[error(transparent)]
	Fatal(FatalKaboom),
	#[error(transparent)]
	Boring(BoringKaboom),
}

impl Fatality for Kaboom {
	pub is_fatal(&self) {
		::core::matches!(self, Self::Fatal)
	}
}
"###);

	}
}
