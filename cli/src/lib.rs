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

//! Polkadot CLI library.

#![warn(missing_docs)]
#![warn(unused_extern_crates)]

mod chain_spec;
#[cfg(feature = "browser")]
mod browser;
#[cfg(feature = "cli")]
mod cli;
#[cfg(feature = "cli")]
mod command;

pub use service::{
	AbstractService, ProvideRuntimeApi, CoreApi, ParachainHost, IsKusama,
	Block, self, RuntimeApiCollection, TFullClient
};

#[cfg(feature = "cli")]
pub use cli::*;

#[cfg(feature = "cli")]
pub use command::*;

pub use chain_spec::*;

#[cfg(feature = "cli")]
pub use sc_cli::{VersionInfo, error};
