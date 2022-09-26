// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Runtime API implementations for Parachains.
//!
//! These are exposed as different modules using different sets of primitives.
//! At the moment there is a v2 module for the current stable api and
//! vstaging module for all staging methods.
//! When new version of the stable api is released it will be based on v2 and
//! will contain methods from vstaging.
//! The promotion consists of the following steps:
//! 1. Bump the version of the stable module (e.g. v2 becomes v3)
//! 2. Move methods from vstaging to v3. The new stable version should include
//!    all methods from vstaging tagged with the new version number (e.g. all
//!    v3 methods).
pub mod v2;
pub mod vstaging;
