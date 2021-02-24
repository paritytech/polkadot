use proc_macro2::{Group, TokenStream, TokenTree};
use quote::{quote, ToTokens};
use syn::{parse2, Attribute, AttributeArgs, Error, TypePath, Meta, MetaList, Fields, FieldsNamed, FieldsUnnamed, Ident, ItemEnum, NestedMeta, Path, Result, Token, Type, Variant, parse::ParseStream, parse::{Parse, Parser}, punctuated::Punctuated};

#[proc_macro_attribute]
pub fn subsystem_dispatch_gen(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let attr = proc_macro2::TokenStream::from(attr);
	let item = proc_macro2::TokenStream::from(item);
	proc_macro::TokenStream::from(subsystem_dispatch_gen2(attr, item))
}

/// An enum variant without base type.
#[derive(Clone)]
struct EnumVariantDispatchWithTy {
	// enum ty name
	ty: Ident,
	// variant
	variant: EnumVariantDispatch,
}

impl ToTokens for EnumVariantDispatchWithTy {
	fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
		if let Some(inner) = &self.variant.inner {
			let enum_name = &self.ty;
			let variant_name = &self.variant.name;

			let ts = quote! {
				#enum_name::#variant_name(#inner::from(event))
			};

			tokens.extend(std::iter::once(ts));
		}
	}
}

/// An enum variant without the base type, contains the relevant inner type.
#[derive(Clone)]
struct EnumVariantDispatch {
	// variant name
	name: Ident,
	// inner type, if any, `None` if skipped
	// TODO make this a TokenStream and only accept verbatim
	inner: Option<Type>,
}


fn prepare_enum_variant(variant: Variant) -> Result<EnumVariantDispatch> {
	let skip = variant.attrs.iter().find(|attr| attr.path.is_ident("skip")).is_some();

	let inner =
		match variant.fields {
			// look for one called inner
			Fields::Named(FieldsNamed { brace_token: _, named }) if !skip => named
				.iter()
				.filter(|field| {
					if let Some(ident) = &field.ident {
						ident.to_string() == "inner".to_owned()
					} else {
						false
					}
				})
				.map(|field| Some(field.ty.clone()))
				.next()
				.unwrap(),
				// .ok_or_else(|| input.error("Missing field named `inner`"))?,

			// take the first one
			Fields::Unnamed(FieldsUnnamed { paren_token: _, unnamed }) if !skip => unnamed
				.first()
				.map(|field| Some(field.ty.clone())).unwrap(),
				// .ok_or_else(|| input.error("Must at least have one inner type or be skipped"))?,
			_ if skip => None,
			_ => {
				// return Err(
				// 	input.error("Enum variant has either not skip and doesn't match anything, or somethin worse")
				// )
				unreachable!("fooo")
			}
		};

	Ok(EnumVariantDispatch { name: variant.ident, inner })
}

fn impl_subsystem_dispatch_gen2(attr: TokenStream, item: TokenStream) -> Result<proc_macro2::TokenStream> {
	let event_ty = dbg!(parse2::<Path>(dbg!(attr))?);

	// let attr = if attrs.len() != 1 {
	// 	unreachable!("FIXME")
	// } else {
	// 	attrs.into_iter().next().expect("Just checked that this is true. qed")
	// };
	// let event_ty: Path = match attrs {
	// 	NestedMeta::Lit(lit) => return Err(Error::new(lit.span(), "Requires exactly one type argument, not a literal")),
	// 	NestedMeta::Meta(Meta::List(MetaList {
	// 		path,
	// 		paren_token: _,
	// 		nested,
	// 	})) => {
	// 		if nested.len() != 2 {
	// 			// return Err(Error::new(lit.span(), "Requires exactly one type argument"))
	// 			unreachable!("ggggg")
	// 		}
	// 		let mut path = None;
	// 		for item in nested {
	// 			match dbg!(item) {
	// 				NestedMeta::Lit(lit) => return Err(Error::new(lit.span(), "Requires exactly one type argument, not a literal")),
	// 				NestedMeta::Meta(Meta::Path(path2)) => {
	// 					path = Some(path2);
	// 				}
	// 				_ => unreachable!("foo_inner"),
	// 			}
	// 		}
	// 		path.expect("Path is set at this point. qed")
	// 	},
	// 	_ => unreachable!("foo_outer"),
	// };

	let ie = dbg!(parse2::<ItemEnum>(dbg!(item))?);

	let message_enum = ie.ident;
	let variants = ie.variants.into_iter()
		.try_fold(Vec::new(), |mut acc: Vec<EnumVariantDispatchWithTy>, variant: Variant| {
			let variant = prepare_enum_variant(variant)?;
			if variant.inner.is_some() {
				acc.push(EnumVariantDispatchWithTy { ty: message_enum.clone(), variant })
			}
			Ok::<_, syn::Error>(acc)
		})?;

	Ok(quote! {
		impl #message_enum {
			pub fn dispatch_iter(event: #event_ty) -> impl IntoIterator<Item=Self> {

				let mut iter = None.into_iter();

				#(
				let mut iter = iter.chain(std::iter::once(event.focus().ok().map(|event| {
						#variants
					})));

				)*
				iter.filter_map(|x| x)
			}
		}
	})
}

fn subsystem_dispatch_gen2(attr: proc_macro2::TokenStream, item: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
	impl_subsystem_dispatch_gen2(attr, item).unwrap_or_else(|err| err.to_compile_error()).into()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pass() {
		let attr = quote! {
			NetEvent<foo::Bar>
		};

		let item = quote! {
			/// Documentation.
			#[derive(Clone)]
			enum AllMessages {

				Sub1(Inner1),

				#[skip]
				/// D3
				Sub3,

				/// D4
				#[skip]
				Sub4(Inner2),

				/// D2
				Sub2(Inner2),
			}
		};

		let output = dbg!(impl_subsystem_dispatch_gen2(attr, item).unwrap());
		println!("//generated:");
		println!("{}", output);
	}
}
