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

#[cfg(feature = "cli")]
mod cli;
#[cfg(feature = "cli")]
mod command;
#[cfg(feature = "cli")]
mod error;
#[cfg(all(feature = "hostperfcheck", build_type = "release"))]
mod host_perf_check;

#[cfg(feature = "full-node")]
pub use service::RuntimeApiCollection;
#[cfg(feature = "service")]
pub use service::{self, Block, CoreApi, IdentifyVariant, ProvideRuntimeApi, TFullClient};

#[cfg(feature = "malus")]
pub use service::overseer::prepared_overseer_builder;

#[cfg(feature = "cli")]
pub use cli::*;

#[cfg(feature = "cli")]
pub use command::*;

#[cfg(feature = "cli")]
pub use sc_cli::{Error, Result};
