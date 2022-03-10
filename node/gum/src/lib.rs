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

//! A wrapper around `tracing` macros, to provide semi automatic
//! `traceID` annotation without codebase turnover.

#[macro_use]
pub use tracing::{Level, event};

pub use jaeger::hash_to_trace_identifier;

pub use gum_proc_macro::{error, warn, info, debug, trace};

#[cfg(test)]
mod tests {
	use super::*;


	#[derive(Default, Debug)]
	struct Y {
		x: u8,
	}

	#[test]
	fn t01() {
		let a = 7;
		info!(target: "foo",
            "Something something {a}, {b:?}, or maybe {c}",
            b = Y::default(),
            c = a
        );
	}

	#[test]
	fn t02() {
		let a = 7;
		info!(target: "bar",
            a = a,
            b = ?Y::default(),
            "fff {c}",
            c = a,
        );
	}

	#[test]
	fn t03() {
		let a = 7;
		info!(target: "bar",
            a = a,
            candidate_hash = 0xFFFFFFF,
            b = ?Y::default(),
            c = ?a,
            "xxx",
		);
	}
}
