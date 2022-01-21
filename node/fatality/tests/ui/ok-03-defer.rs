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

pub use fatality::fatality;

#[derive(Debug, thiserror::Error)]
#[fatal("We tried")]
struct Fatal;

#[derive(Debug, thiserror::Error)]
#[error("Get a dinosaur bandaid")]
struct Bobo;

#[fatality]
enum Kaboom {
	#[defer(transparent)]
	Iffy(Fatal),

	#[error(transparent)]
	Bobo(Bobo),
}

fn iffy() -> Result<(), Kaboom> {
	Ok(Err(Fatal)?)
}

fn bobo() -> Result<(), Kaboom> {
	Ok(Err(Bobo)?)
}

fn main() {
	if iffy().is_fatal() {
		assert_matches!(fatal, Kaboom::Iffy(_));
	}
	if bobo().is_fatal() {
		assert_matches!(bobo, BoringKaboom::Bobo(_));
	}
}
