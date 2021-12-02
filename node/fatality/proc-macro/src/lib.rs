use proc_macro2::{Ident, TokenStream, Span};
use quote::{quote, ToTokens};
use syn::parse::Parse;
use syn::{Variant, ItemEnum, Path, parse2};
use syn::spanned::Spanned;

// pub mod keywords {
// 	syn::custom_keyword!(fatal);
// }



// enum Fatality {
// 	Fatal,
// 	Boring,
// }

// impl Parse for IsFatal {
//     fn parse(input: ParseStream) -> Result<Fatality> {
//         let lookahead = input.lookahead1();
//         if lookahead.peek(keywords::fatal) {
// 			let _ = input.parse::<kw::fatal>()?;
//             Ok(Argument::Fatal)
//         } else if lookahead.peek(kw::str) {
//             Ok(Argument::Str {
//                 str_token: input.parse::<kw::str>()?,
//                 eq_token: input.parse()?,
//                 value: input.parse()?,
//             })
//         } else {
//             Err(lookahead.error())
//         }
//     }
// }

use proc_macro_crate::{crate_name, FoundCrate};

fn abs(what: impl Into<Path>, loco: Span) -> Path {
	let what = what.into();
    let found_crate = if cfg!(test) {
		FoundCrate::Itself
	} else { crate_name("fatality").expect("`fatality` is present in `Cargo.toml`") };
	let ts = match found_crate {
        FoundCrate::Itself => quote!( crate::#what ),
        FoundCrate::Name(name) => {
            let ident = Ident::new(&name, loco);
            quote!( #ident::#what )
        }
    };
	let path: Path  = parse2(ts).unwrap();
	path
}

fn trait_fatality_impl(who: Ident, logic: TokenStream) -> TokenStream {
	let fatality_trait = abs(Ident::new("Fatality", who.span()), who.span());
	quote!{
		impl #fatality_trait for #who {
			fn is_fatal(&self) -> bool {
				#logic
			}
		}
	}
}

fn gen(item: &ItemEnum) -> TokenStream {
	let mut item2 = item.clone();
	let name = item.ident.clone();

	let mut fatal_variants = Vec::<Variant>::new();
	let mut jfyi_variants = Vec::<Variant>::new();
	// if there is not a single fatal annotation, we can just replace `#[fatality]` with `3[thiserror::Error]`
	// without the intermediate type. But impl `trait Fatality` on-top.
	let mut fatal_count = 0;
	for variant in item2.variants.iter_mut() {
		dbg!(&variant);
		let mut is_fatal = false;
		while let Some(idx) = variant.attrs
			.iter().enumerate()
			.find_map(|(idx, attr)| {
				if dbg!(&attr.path).is_ident(&Ident::new("fatal", Span::call_site())) {
					Some(idx)
				} else {
					None
				}
			})
		{
			dbg!(&mut variant.attrs).swap_remove(idx);
			is_fatal = true;
		}
		if is_fatal {
			fatal_count += 1;
			fatal_variants.push(variant.clone());
		} else {
			jfyi_variants.push(variant.clone());
		}
	}

	let fatal_only = fatal_count == item2.variants.len();
	let jfyi_only = fatal_count == 0;

	// we can avoid the entire generation of extra enums if none, or all variants are fatal or jfyi
	if !fatal_only && !jfyi_only {
		let name_fatal = Ident::new(format!("Fatal{}", name).as_str(), item.span());
		let name_jfyi = Ident::new(format!("Jfyi{}", name).as_str(), item.span());
		let vis = item.vis.clone();

		let thiserror: Path = parse2(quote!(thiserror::Error)).unwrap();
		let thiserror = abs(thiserror, name.span());
		let wrapper_enum = quote! {
			#[#thiserror]
			#vis enum #name {
				#[error(transparent)]
				Fatal(#name_fatal),
				#[error(transparent)]
				Jfyi(#name_jfyi),
			}
		};
		let jfyi_enum = quote! {
			#[#thiserror]
			#vis enum #name_jfyi {
				#( #jfyi_variants ),*
			}
		};
		let fatal_enum = quote! {
			#[#thiserror]
			#vis enum #name_fatal {
				#( #fatal_variants ),*
			}
		};
		let mut ts = TokenStream::new();
		ts.extend(wrapper_enum);
		ts.extend(trait_fatality_impl(name, quote! {
			match self {
				Self::Fatal(_) => true,
				Self::Jfyi(_) => false,
			}
		}));

		ts.extend(fatal_enum);
		ts.extend(trait_fatality_impl(name_fatal, quote! {
			true
		}));

		ts.extend(jfyi_enum);
		ts.extend(trait_fatality_impl(name_jfyi, quote! {
			// TODO, if there is a `#[source]` annotation, and `jfyi(fwd)` annotation, fwd to the inner
			false
		}));
		ts
	} else {
		let mut ts = item.to_token_stream();
		ts.extend(
		trait_fatality_impl(name, quote! {
				#fatal_only
			})
		);
		ts
	}

}

fn fatality2(input: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
	let mut item: ItemEnum = match syn::parse2(input.clone()) {
		Err(e) => {
			let mut bail = input.into_token_stream();
			bail.extend(e.to_compile_error());
			return bail
		}
		Ok(item) => item,
	};
	let res = gen(&item);
	res
}

#[proc_macro]
pub fn fatality(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let input = TokenStream::from(input);

    let output: TokenStream = fatality2(input);

    proc_macro::TokenStream::from(output)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn basic_full() {
		let input = quote!{
enum Kaboom {
	#[fatal, error(transparent)]
	A(X),
	#[error(transparent)]
	B(Y),
}
		};
		let output = fatality2(dbg!(input));
		println!(r##">>>>>>>>>>>>>>>>>>>
{}
>>>>>>>>>>>>>>>>>>>"##, output.to_string());
		assert_eq!(output.to_string(), quote!{
#[crate::thiserror::Error]
enum Kaboom {
	#[error(transparent)]
	Fatal(FatalKaboom),
	#[error(transparent)]
	Jfyi(JfyiKaboom),
}


impl crate::Fatality for Kaboom {
	fn is_fatal(&self) -> bool {
		match self {
			Self::Fatal(_) => true,
			Self::Jfyi(_) => false,
		}
	}
}

#[crate::thiserror::Error]
enum FatalKaboom {
	#[error(transparent)]
	A(X),
}

impl crate::Fatality for FatalKaboom {
	fn is_fatal(&self) -> bool {
		true
	}
}

#[crate::thiserror::Error]
enum JfyiKaboom {
	#[error(transparent)]
	B(Y),
}

impl crate::Fatality for JfyiKaboom {
	fn is_fatal(&self) -> bool {
		false
	}
}

	}.to_string());
}
}
