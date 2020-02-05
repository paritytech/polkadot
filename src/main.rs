// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot CLI

#![warn(missing_docs)]

use cli::VersionInfo;

fn main() -> Result<(), cli::error::Error> {
	let version = VersionInfo {
		name: "Parity Polkadot",
		commit: env!("VERGEN_SHA_SHORT"),
		version: env!("CARGO_PKG_VERSION"),
		executable_name: "polkadot",
		author: "Parity Team <admin@parity.io>",
		description: "Polkadot Relay-chain Client Node",
		support_url: "https://github.com/paritytech/polkadot/issues/new",
		copyright_start_year: 2017,
	};

	cli::run(version)
}
