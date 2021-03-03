// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Relaying source chain headers to target chain. This module provides entrypoint
//! that starts reading new headers from source chain and submit these headers as
//! module/contract transactions to the target chain. Module/contract on the target
//! chain is a light-client of the source chain. All other trustless bridge
//! applications are built using this light-client, so running headers-relay is
//! essential for running all other bridge applications.

// required for futures::select!
#![recursion_limit = "1024"]
#![warn(missing_docs)]

pub mod headers;
pub mod sync;
pub mod sync_loop;
pub mod sync_loop_metrics;
pub mod sync_loop_tests;
pub mod sync_types;
