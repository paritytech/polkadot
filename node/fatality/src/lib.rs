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

pub use fatality_proc_macro::fatality;
pub use thiserror;

/// Determine the fatality of an error.
pub trait Fatality: std::error::Error + std::fmt::Debug {
	fn is_fatal(&self) -> bool;
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
