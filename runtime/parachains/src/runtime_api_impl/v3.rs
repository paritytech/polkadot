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

//! A module exporting runtime API implementation functions for all runtime APIs using `v3`
//! primitives.
//!
//! Runtimes implementing the `v3` runtime API are recommended to forward directly to these
//! functions.

use crate::dmp;
use primitives::v2::{Id, InboundDownwardMessage};
use sp_std::prelude::*;

/// Implementation for the `dmq_contents_bounded` function of the runtime API.
pub fn dmq_contents_bounded<T: dmp::Config>(
	parachain_id: Id,
	start: u32,
	count: u32,
) -> Vec<InboundDownwardMessage<T::BlockNumber>> {
	<dmp::Pallet<T>>::dmq_contents_bounded(parachain_id, start, count)
}
