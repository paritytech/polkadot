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
pub trait Fatality: std::error::Error + Debug {
	fn is_fatal(&self) -> bool;
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Debug, thiserror::Error)]
	#[error("X")]
	struct X;

	#[derive(Debug, thiserror::Error)]
	#[error("Y")]
	struct Y;

	#[fatality]
	enum Acc {
		#[fatal(source)]
		#[error("X={0}")]
		A(#[source] X),

		#[error(transparent)]
		B(Y),
	}

	#[test]
	fn all_in_one() {
		// TODO this must continue to work, so consider retaining the original error type as is, and only split
		// TODO on `is_fatal() -> Result<Jfyi, Fatal>`
		assert_eq!(true, Fatality::is_fatal(&Acc::A(X)));
		assert_eq!(false, Fatality::is_fatal(&Acc::B(Y)));
	}
}
