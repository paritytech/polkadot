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

//! Wrappers around creating a signer account.

use crate::prelude::*;
use subxt::{sp_core::Pair as _, PairSigner};

/// Read the signer account's URI
pub(crate) fn signer_from_string(mut seed_or_path: &str) -> Result<Signer, Error> {
	seed_or_path = seed_or_path.trim();

	let seed = match std::fs::read(seed_or_path) {
		Ok(s) => String::from_utf8(s).map_err(|e| Error::Other(e.to_string()))?,
		Err(_) => seed_or_path.to_string(),
	};
	let seed = seed.trim();

	let pair = Pair::from_string(seed, None).map_err(|e| Error::Crypto(e))?;

	Ok(PairSigner::new(pair))
}
