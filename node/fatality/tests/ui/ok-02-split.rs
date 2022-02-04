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

use fatality::{fatality, Split};
use assert_matches::assert_matches;

#[derive(Debug, thiserror::Error)]
#[error("We tried")]
struct Fatal;

#[derive(Debug, thiserror::Error)]
#[error("Get a dinosaur bandaid")]
struct Bobo;

#[fatality(splitable)]
enum Kaboom {
	#[fatal]
	#[error(transparent)]
	Iffy(#[from] Fatal),

	#[error(transparent)]
	Bobo(#[from] Bobo),
}


fn iffy() -> Result<(), Kaboom> {
	Err(Fatal)?
}

fn bobo() -> Result<(), Kaboom> {
	Err(Bobo)?
}

fn main() {
	if let Err(fatal) = iffy().unwrap_err().split() {
		assert_matches!(fatal, FatalKaboom::Iffy(_));
	}
	if let Ok(bobo) = bobo().unwrap_err().split() {
		assert_matches!(bobo, JfyiKaboom::Bobo(_));
	}
}
