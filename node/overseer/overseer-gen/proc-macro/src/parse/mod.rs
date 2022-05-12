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

mod kw {
	syn::custom_keyword!(event);
	syn::custom_keyword!(signal);
	syn::custom_keyword!(error);
	syn::custom_keyword!(outgoing);
	syn::custom_keyword!(gen);
	syn::custom_keyword!(signal_capacity);
	syn::custom_keyword!(message_capacity);
	syn::custom_keyword!(subsystem);
	syn::custom_keyword!(prefix);
}

mod parse_overseer_attr;
mod parse_overseer_struct;

mod parse_subsystem_attr;

#[cfg(test)]
mod tests;

pub(crate) use self::{parse_overseer_attr::*, parse_overseer_struct::*};

pub(crate) use self::parse_subsystem_attr::*;
