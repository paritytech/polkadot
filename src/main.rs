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

use color_eyre::eyre;

use cli::Error as PolkaError;

use std::{error, fmt};

/// A helper to satisfy the requirements of `eyre`
/// compatible errors, which require `Send + Sync`
/// which are not satisfied by the `sp_*` crates.
#[derive(Debug)]
struct ErrorWrapper(std::sync::Arc<PolkaError>);

// nothing is going to be sent to another thread
// it merely exists to glue two distinct error
// types together where the requirements differ
// with `Sync + Send` and without them for `wasm`.
unsafe impl Sync for ErrorWrapper {}
unsafe impl Send for ErrorWrapper {}

impl error::Error for ErrorWrapper {
	fn source(&self) -> Option<&(dyn error::Error + 'static)> {
		(&*self.0).source().and_then(|e| e.source())
	}
	fn description(&self) -> &str {
		"Error Wrapper"
	}
}

impl fmt::Display for ErrorWrapper {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", &*self.0)
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	cli::run().map_err(|e| ErrorWrapper(std::sync::Arc::new(e)))?;
	Ok(())
}
