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

//! Types used to connect to the Kusama chain.

use relay_substrate_client::{Chain, ChainBase};
use std::time::Duration;

/// Kusama header id.
pub type HeaderId = relay_utils::HeaderId<bp_kusama::Hash, bp_kusama::BlockNumber>;

/// Kusama chain definition
#[derive(Debug, Clone, Copy)]
pub struct Kusama;

impl ChainBase for Kusama {
	type BlockNumber = bp_kusama::BlockNumber;
	type Hash = bp_kusama::Hash;
	type Hasher = bp_kusama::Hasher;
	type Header = bp_kusama::Header;
}

impl Chain for Kusama {
	const NAME: &'static str = "Kusama";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);

	type AccountId = bp_kusama::AccountId;
	type Index = bp_kusama::Nonce;
	type SignedBlock = bp_kusama::SignedBlock;
	type Call = ();
}

/// Kusama header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_kusama::Header>;
