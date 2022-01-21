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

mod basic {
	use super::*;
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
							Self::V(..) => true
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
							Self::V(ref inner, ..) => inner.is_fatal()
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
							Self::V(..) => true
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
							Self::V(_, ref inner, ..) => inner.is_fatal()
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
							Self::O(..) => true
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
				(splitable)
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
							Self::A(ref inner, ..) => inner.is_fatal(),
							Self::B(ref inner, ..) => inner.is_fatal(),
							Self::C{ref z, ..} => z.is_fatal(),
							Self::What => false,
							Self::O(..) => true
						}
					}
				}

				impl From<JfyiKaboom> for Kaboom {
					fn from(jfyi: JfyiKaboom) -> Self {
						match jfyi {
							JfyiKaboom::Eh => Kaboom::Eh,
						}
					}
				}


				impl From<FatalKaboom> for Kaboom {
					fn from(fatal: FatalKaboom) -> Self {
						match fatal {
							FatalKaboom::Explosion => Kaboom::Explosion,
						}
					}
				}

				#[derive(crate::thiserror::Error, Debug)]
				enum JfyiKaboom {
					#[error("Eh?")]
					Eh,
				}

				#[derive(crate::thiserror::Error, Debug)]
				enum FatalKaboom {
					#[error("Explosion")]
					Explosion,
				}

				impl crate::Split for Kaboom {
					type Jfyi = JfyiKaboom;
					type Fatal = FatalKaboom;

					fn split(self) -> ::std::result::Result<Self::Jfyi, Self::Fatal> {
						match self {
							// jfyi
							Self::Eh => Ok(<Self::Fatal>::Eh),
							// fatal
							Self::Explosion => Err(<Self::Fatal>::Explosion)
						}
					}
				}
			},
		);
	}
}
