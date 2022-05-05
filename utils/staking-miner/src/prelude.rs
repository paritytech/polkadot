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

use codec::{Decode, Encode};

#[derive(codec::Encode, codec::Decode)]
pub struct SubmitCall<T>(Box<pallet_election_provider_multi_phase::RawSolution<T>>);

impl<T> SubmitCall<T> {
	pub fn new(raw_solution: pallet_election_provider_multi_phase::RawSolution<T>) -> Self {
		Self(Box::new(raw_solution))
	}
}

impl<T: Encode> subxt::Call for SubmitCall<T> {
	const PALLET: &'static str = "ElectionProviderMultiPhase";
	const FUNCTION: &'static str = "submit";
}

#[derive(Encode, Decode, Debug)]
pub struct ModuleErrMissing;

impl subxt::HasModuleError for ModuleErrMissing {
	fn module_error_data(&self) -> Option<subxt::ModuleErrorData> {
		None
	}
}

pub type NoEvents = String;

/// The account id type.
pub type AccountId = subxt::sp_core::crypto::AccountId32;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Header = subxt::sp_runtime::generic::Header<u32, subxt::sp_runtime::traits::BlakeTwo256>;
/// The header type. We re-export it here, but we can easily get it from block as well.
pub type Hash = subxt::sp_core::H256;

pub use subxt::{
	sp_core,
	sp_runtime::traits::{Block as BlockT, Header as HeaderT},
};

/// Default URI to connect to.
pub const DEFAULT_URI: &str = "wss://rpc.polkadot.io:443";
/// The logging target.
pub const LOG_TARGET: &str = "staking-miner";

/// The key pair type being used. We "strongly" assume sr25519 for simplicity.
pub type Pair = subxt::sp_core::sr25519::Pair;

/// Type alias for the subxt client.
pub type SubxtClient = subxt::Client<subxt::DefaultConfig>;

/// Runtime API.
pub type RuntimeApi = crate::runtime::RuntimeApi<
	subxt::DefaultConfig,
	subxt::SubstrateExtrinsicParams<subxt::DefaultConfig>,
>;

pub type ExtrinsicParams = subxt::SubstrateExtrinsicParams<subxt::DefaultConfig>;

pub use crate::{error::Error, runtime::runtime_types as runtime};
pub use pallet_election_provider_multi_phase::{Miner, MinerConfig};

pub type Signer = subxt::PairSigner<subxt::DefaultConfig, subxt::sp_core::sr25519::Pair>;

pub use sp_runtime::Perbill;
