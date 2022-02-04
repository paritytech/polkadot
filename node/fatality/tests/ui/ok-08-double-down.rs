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

use fatality::{fatality, Fatality, Split};
use assert_matches::assert_matches;

#[fatality(splitable)]
enum Wormhole {
	#[fatal]
	#[error("Ice baby")]
	Neptun,

	#[error("So close")]
	Moon,
}

#[fatality(splitable)]
enum Inner {
	#[fatal(forward)]
    #[error(transparent)]
	Wormhole(Wormhole),

	#[error("Abyss")]
	Abyss,
}


#[fatality(splitable)]
enum Kaboom {
	#[fatal(forward)]
    #[error(transparent)]
	Iffy(Inner),

	#[error("Bobo")]
	Bobo,
}

fn neptun() -> Result<(), Kaboom> {
	Err(Kaboom::Iffy(Inner::Wormhole(Wormhole::Neptun)))
}

fn moon() -> Result<(), Kaboom> {
    Err(Kaboom::Iffy(Inner::Wormhole(Wormhole::Moon)))
}

fn main() {
	assert!(neptun().unwrap_err().is_fatal());
    assert_matches!(neptun().unwrap_err().split(), Err(FatalKaboom::Iffy(..)));
	assert_matches!(neptun().unwrap_err().split(), Err(FatalKaboom::Iffy(Inner::Wormhole(..))));

    assert!(!moon().unwrap_err().is_fatal());
    assert_matches!(moon().unwrap_err().split(), Ok(JfyiKaboom::Iffy(..)));
	assert_matches!(moon().unwrap_err().split(), Ok(JfyiKaboom::Iffy(Inner::Wormhole(..))));
}
