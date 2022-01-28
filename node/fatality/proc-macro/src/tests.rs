// Copyright 2022 Parity Technologies (UK) Ltd.
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

use super::*;
use assert_matches::assert_matches;

fn run_test(attr: TokenStream, input: TokenStream, expected: TokenStream) {
	let output = fatality2(attr, input);
	let output = output.to_string();
	println!(
		r##">>>>>>>>>>>>>>>>>>>
{}
>>>>>>>>>>>>>>>>>>>"##,
		output.as_str()
	);
	assert_eq!(output, expected.to_string(),);
}

mod component {
	use super::*;

	#[test]
	fn parse_attr_blank() {
		let input = TokenStream::new();
		let result = syn::parse2::<Attr>(input);
		assert_matches!(result, Ok(_));
	}

	#[test]
	fn parse_attr_splitable() {
		let input = quote! { splitable }.into();
		let result = syn::parse2::<Attr>(input);
		assert_matches!(result, Ok(_));
	}

	#[test]
	fn parse_attr_err() {
		let input = quote! { xyz }.into();
		let result = syn::parse2::<Attr>(input);
		assert_matches!(result, Err(_));
	}
}

mod basic {
	use super::*;

	#[test]
	fn visibility_pub_crate_is_retained() {
		run_test(
			TokenStream::new(),
			quote! {
				pub(crate) enum Q {
					#[fatal]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				pub(crate) enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(..) => true,
						}
					}
				}
			},
		);
	}

	#[test]
	fn transparent_fatal_implicit() {
		run_test(
			TokenStream::new(),
			quote! {
				enum Q {
					#[fatal]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(..) => true,
						}
					}
				}
			},
		);
	}

	#[test]
	fn transparent_fatal_fwd() {
		run_test(
			TokenStream::new(),
			quote! {
				enum Q {
					#[fatal(forward)]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]

				enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(ref inner, ..) => inner.is_fatal(),
						}
					}
				}
			},
		);
	}

	#[test]
	fn transparent_fatal_true() {
		run_test(
			TokenStream::new(),
			quote! {
				enum Q {
					#[fatal(true)]
					#[error(transparent)]
					V(I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Q {
					#[error(transparent)]
					V(I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(..) => true,
						}
					}
				}
			},
		);
	}

	#[test]
	fn source_fatal() {
		run_test(
			TokenStream::new(),
			quote! {
				enum Q {
					#[fatal(forward)]
					#[error("DDDDDDDDDDDD")]
					V(first, #[source] I),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]

				enum Q {
					#[error("DDDDDDDDDDDD")]
					V(first, #[source] I),
				}


				impl crate::Fatality for Q {
					fn is_fatal(&self) -> bool {
						match self {
							Self::V(_, ref inner, ..) => inner.is_fatal(),
						}
					}
				}
			},
		);
	}

	#[test]
	fn full() {
		run_test(
			TokenStream::new(),
			quote! {
				enum Kaboom {
					#[fatal(forward)]
					#[error(transparent)]
					// only one arg, that's ok, the first will be used
					A(X),

					#[fatal(forward)]
					#[error("Bar")]
					B(#[source] Y),

					#[fatal(forward)]
					#[error("zzzZZzZ")]
					C {#[source] z: Z },

					#[error("What?")]
					What,


					#[fatal]
					#[error(transparent)]
					O(P),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Kaboom {
					#[error(transparent)]
					A(X),
					#[error("Bar")]
					B(#[source] Y),
					#[error("zzzZZzZ")]
					C {#[source] z: Z },
					#[error("What?")]
					What,
					#[error(transparent)]
					O(P),
				}

				impl crate::Fatality for Kaboom {
					fn is_fatal(&self) -> bool {
						match self {
							Self::A(ref inner, ..) => inner.is_fatal(),
							Self::B(ref inner, ..) => inner.is_fatal(),
							Self::C{ref z, ..} => z.is_fatal(),
							Self::What => false,
							Self::O(..) => true,
						}
					}
				}
			},
		);
	}
}

mod splitable {
	use super::*;

	#[test]
	fn simple() {
		run_test(
			quote! {
				splitable
			},
			quote! {
				enum Kaboom {
					#[error("Eh?")]
					Eh,

					#[fatal]
					#[error("Explosion")]
					Explosion,
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				enum Kaboom {
					#[error("Eh?")]
					Eh,
					#[error("Explosion")]
					Explosion,
				}

				impl crate::Fatality for Kaboom {
					fn is_fatal(&self) -> bool {
						match self {
							Self::Eh => false,
							Self::Explosion => true,
						}
					}
				}

				impl ::std::convert::From<FatalKaboom> for Kaboom {
					fn from(fatal: FatalKaboom) -> Self {
						match fatal {
							FatalKaboom::Explosion => Self::Explosion,
						}
					}
				}

				impl ::std::convert::From<JfyiKaboom> for Kaboom {
					fn from(jfyi: JfyiKaboom) -> Self {
						match jfyi {
							JfyiKaboom::Eh => Self::Eh,
						}
					}
				}


				#[derive(crate::thiserror::Error, Debug)]
				enum FatalKaboom {
					#[error("Explosion")]
					Explosion
				}

				#[derive(crate::thiserror::Error, Debug)]
				enum JfyiKaboom {
					#[error("Eh?")]
					Eh
				}

				impl crate::Split for Kaboom {
					type Fatal = FatalKaboom;
					type Jfyi = JfyiKaboom;

					fn split(self) -> ::std::result::Result<Self::Jfyi, Self::Fatal> {
						match self {
							// Fatal
							Self::Explosion => Err(FatalKaboom::Explosion),
							// JFYI
							Self::Eh => Ok(JfyiKaboom::Eh),
						}
					}
				}
			},
		);
	}

	#[test]
	fn regression() {
		run_test(
			quote! {
				splitable
			},
			quote! {
				pub enum X {
					#[fatal]
					#[error("Cancelled")]
					Inner(Foo),
				}
			},
			quote! {
				#[derive(crate::thiserror::Error, Debug)]
				pub enum X {
					#[error("Cancelled")]
					Inner(Foo),
				}

				impl crate :: Fatality for X {
					fn is_fatal (& self) -> bool {
						match self {
							Self :: Inner (..) => true ,
						}
					}
				}

				impl :: std :: convert :: From < FatalX > for X {
					fn from (fatal : FatalX) -> Self {
						match fatal {
							FatalX :: Inner(arg_0) => Self :: Inner(arg_0),
						}
					}
				}

				impl :: std :: convert :: From < JfyiX > for X {
					fn from (jfyi : JfyiX) -> Self {
						match jfyi {

						}
					}
				}

				# [derive (crate :: thiserror :: Error , Debug)]
				pub enum FatalX {
					#[error("Cancelled")]
					Inner (Foo) }
					#[derive (crate :: thiserror :: Error , Debug)]
					pub enum JfyiX { }
					impl crate :: Split for X {
						type Fatal = FatalX ;
						type Jfyi = JfyiX ;
						fn split (self) -> :: std :: result :: Result < Self :: Jfyi , Self :: Fatal > {
							match self {
								Self::Inner(arg_0) => Err (FatalX :: Inner(arg_0)) ,
							}
						}
					}
			},
		);
	}
}
