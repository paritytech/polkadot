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

//! Types that we don't fetch from a particular runtime and just assume that they are constant all
//! of the place.
//!
//! It is actually easy to convert the rest as well, but it'll be a lot of noise in our codebase,
//! needing to sprinkle `any_runtime` in a few extra places.

/// The account id type.
pub type AccountId = core_primitives::AccountId;
/// The block number type.
pub type BlockNumber = core_primitives::BlockNumber;
/// The balance type.
pub type Balance = core_primitives::Balance;
/// The index of an account.
pub type Index = core_primitives::AccountIndex;
/// The hash type. We re-export it here, but we can easily get it from block as well.
pub type Hash = core_primitives::Hash;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header = core_primitives::Header;

pub use sp_runtime::traits::{Block as BlockT, Header as HeaderT};

/// Default URI to connect to.
pub const DEFAULT_URI: &str = "wss://rpc.polkadot.io:443";
/// The logging target.
pub const LOG_TARGET: &str = "staking-miner";

/// The election provider pallet.
pub use pallet_election_provider_multi_phase as EPM;

/// The externalities type.
pub type Ext = sp_io::TestExternalities;

/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = sp_core::sr25519::Pair;

/// A dynamic token type used to represent account balances.
pub type Token = sub_tokens::dynamic::DynamicToken;
