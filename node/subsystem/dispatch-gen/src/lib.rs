use proc_macro2::{Group, TokenStream, TokenTree};
use quote::{quote, ToTokens};
use syn::{
	parse::ParseStream,
	parse::{Parse, Parser},
	punctuated::Punctuated,
	Error, Fields, FieldsNamed, FieldsUnnamed, Ident, Result, Token, Type, Variant,
};

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

impl Parse for EnumVariantDispatch {
	fn parse(input: ParseStream) -> Result<Self> {
		let variant = Variant::parse(input)?;

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
					.ok_or_else(|| input.error("Missing field named `inner`"))?,

				// take the first one
				Fields::Unnamed(FieldsUnnamed { paren_token: _, unnamed }) if !skip => unnamed
					.first()
					.map(|field| Some(field.ty.clone()))
					.ok_or_else(|| input.error("Must at least have one inner type or be skipped"))?,
				_ if skip => None,
				_ => {
					return Err(
						input.error("Enum variant has either not skip and doesn't match anything, or somethin worse")
					)
				}
			};

		Ok(EnumVariantDispatch { name: variant.ident, inner })
	}
}

fn extract_variants_inner_ty(group: Group) -> Result<Vec<EnumVariantDispatch>> {
	let stream = group.stream();
	dbg!(group);
	let parser = Punctuated::<EnumVariantDispatch, Token![,]>::parse_terminated;
	let variants: Vec<EnumVariantDispatch> =
		parser.parse2(stream)?.into_iter().filter(|variant| variant.inner.is_some()).collect();

	Ok(variants)
}

fn extract_inner_ty(stream: TokenStream) -> Result<TokenStream> {
	Ok(stream)
}

fn impl_subsystem_dispatch_gen2(attr: TokenStream, item: TokenStream) -> Result<proc_macro2::TokenStream> {
	let mut attr = dbg!(attr.clone()).into_iter();
	let event_ty = if let Some(TokenTree::Group(attr_group)) = attr.next() {
		// includes the macro name
		let mut attr = attr_group.stream().into_iter();
		if let Some(x) = attr.next() {
			dbg!(x);
		}
		let event_ty = if let Some(TokenTree::Group(attr_group)) = attr.next() {
			attr_group.stream()
		} else {
			unreachable!("inner")
		};
		event_ty
	} else {
		unreachable!("outer")
	};

	let mut iter = item.clone().into_iter();
	match iter.next() {
		Some(TokenTree::Ident(ident)) if ident.to_string() == "enum" => {}
		Some(other) => return Err(Error::new(other.span(), "Only works on enums")),
		None => unreachable!("z"),
	}
	let message_enum = match iter.next() {
		Some(TokenTree::Ident(ident)) => ident,
		Some(other) => return Err(Error::new(other.span(), "Expected an identifier as type name")),
		None => unreachable!("f"),
	};

	let variants: Vec<_> = match iter.next() {
		Some(TokenTree::Group(group)) => extract_variants_inner_ty(group)?
			.into_iter()
			.map(|variant| EnumVariantDispatchWithTy { ty: message_enum.clone(), variant })
			.collect::<Vec<_>>(),
		Some(other) => return Err(Error::new(other.span(), "Expected a group of items")),
		None => unreachable!("yyyyyy"),
	};

	Ok(quote! {
		impl #message_enum {
			pub fn dispatch_iter(event: #event_ty) -> impl IntoIterator<Item=#message_enum> {

				let mut iter = None.into_iter();

				#(
				let mut iter = iter.chain(std::iter::once(event.focus().ok().map(|event| {
						#variants
					})));

				),*
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
		let attr = dbg!(quote! {
			NetEvent<foo::Bar>
		});

		let item = dbg!(quote! {
			enum AllMessages {
				Sub1(Inner1),

				#[skip]
				Sub3,

				#[skip]
				Sub4(Inner2),

				Sub2(Inner2),
			}
		});

		let output = dbg!(subsystem_dispatch_gen2(attr, item));
		println!("//generated:");
		println!("{}", output);
	}
}
