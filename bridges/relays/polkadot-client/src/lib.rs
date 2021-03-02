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

//! Types used to connect to the Polkadot chain.

use relay_substrate_client::{Chain, ChainBase};
use std::time::Duration;

/// Polkadot header id.
pub type HeaderId = relay_utils::HeaderId<bp_polkadot::Hash, bp_polkadot::BlockNumber>;

/// Polkadot chain definition
#[derive(Debug, Clone, Copy)]
pub struct Polkadot;

impl ChainBase for Polkadot {
	type BlockNumber = bp_polkadot::BlockNumber;
	type Hash = bp_polkadot::Hash;
	type Hasher = bp_polkadot::Hasher;
	type Header = bp_polkadot::Header;
}

impl Chain for Polkadot {
	const NAME: &'static str = "Polkadot";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);

	type AccountId = bp_polkadot::AccountId;
	type Index = bp_polkadot::Nonce;
	type SignedBlock = bp_polkadot::SignedBlock;
	type Call = ();
}

/// Polkadot header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_polkadot::Header>;
