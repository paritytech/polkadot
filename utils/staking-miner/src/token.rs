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

pub use ss58_registry::{Token as WrappedToken, TokenAmount, TokenRegistry};

static mut TOKEN: WrappedToken = WrappedToken { name: "polkadot", decimals: 10 };

pub struct Token;

impl Token {
	pub fn init(token: WrappedToken) {
		// safety: this program will always be single threaded, thus accessing global static is
		// safe.
		unsafe {
			TOKEN = token;
		}
	}

	pub fn from(value: u128) -> TokenAmount {
		// safety: this program will always be single threaded, thus accessing global static is
		// safe.
		unsafe { TOKEN.amount(value) }
	}
}
