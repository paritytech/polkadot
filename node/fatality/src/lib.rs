// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Add annotations of `fatality` to error `enum` types.

pub use fatality_proc_macro::fatality;
pub use thiserror;

/// Determine the fatality of an error.
pub trait Fatality: std::error::Error + std::fmt::Debug {
	/// Returns `true` if the error variant is _fatal_
	/// or `false` if it is more of a informational error.
	fn is_fatal(&self) -> bool;
}

/// Allows to split an error into two types - a fatal
/// and a informational enum error type, that can be further consumed.
pub trait Split: std::error::Error + std::fmt::Debug {
	type Jfyi: std::error::Error + Send + Sync + 'static;
	type Fatal: std::error::Error + Send + Sync + 'static;

	/// Split the error into it's fatal and non-fatal variants.
	///
	/// `Ok(jfyi)` contains a enum representing all non-fatal varians, `Err(fatal)`
	/// contains all fatal variants.
	///
	/// Attention: If the type is splitable, it must _not_ use any `forward`ed finality
	/// evalutions, or it must be splitable up the point where no more `forward` annotations
	/// were used.
	fn split(self) -> std::result::Result<Self::Jfyi, Self::Fatal>;
}

/// Converts a flat, yet `splitable` error into a nested `Result<Result<_,Jfyi>, Fatal>`
/// error type.
pub trait Nested<T, E: Split>
where
	Self: Sized,
{
	/// Convert into a nested error rather than a flat one, commonly for direct handling.
	fn into_nested(
		self,
	) -> std::result::Result<std::result::Result<T, <E as Split>::Jfyi>, <E as Split>::Fatal>;
}

impl<T, E: Split> Nested<T, E> for std::result::Result<T, E> {
	fn into_nested(
		self,
	) -> std::result::Result<std::result::Result<T, <E as Split>::Jfyi>, <E as Split>::Fatal> {
		match self {
			Ok(t) => Ok(Ok(t)),
			Err(e) => match e.split() {
				Ok(jfyi) => Ok(Err(jfyi)),
				Err(fatal) => Err(fatal),
			},
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Debug, thiserror::Error)]
	#[error("X")]
	struct X;

	impl Fatality for X {
		fn is_fatal(&self) -> bool {
			false
		}
	}

	#[derive(Debug, thiserror::Error)]
	#[error("Y")]
	struct Y;

	impl Fatality for Y {
		fn is_fatal(&self) -> bool {
			true
		}
	}

	#[fatality]
	enum Acc {
		#[error("0")]
		Zero,

		#[error("X={0}")]
		A(#[source] X),

		#[fatal]
		#[error(transparent)]
		B(Y),

		#[fatal(forward)]
		#[error("X={0}")]
		Aaaaa(#[source] X),

		#[fatal(forward)]
		#[error(transparent)]
		Bbbbbb(Y),
	}

	#[test]
	fn all_in_one() {
		assert_eq!(false, Fatality::is_fatal(&Acc::A(X)));
		assert_eq!(true, Fatality::is_fatal(&Acc::B(Y)));
		assert_eq!(false, Fatality::is_fatal(&Acc::Aaaaa(X)));
		assert_eq!(true, Fatality::is_fatal(&Acc::Bbbbbb(Y)));
		assert_eq!(false, Fatality::is_fatal(&Acc::Zero));
	}
}
