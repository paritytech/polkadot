// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! Types used to connect to the Rialto-Substrate chain.

use frame_support::weights::Weight;
use relay_substrate_client::{Chain, ChainBase};
use std::time::Duration;

/// Rialto header id.
pub type HeaderId =
	relay_utils::HeaderId<rialto_parachain_runtime::Hash, rialto_parachain_runtime::BlockNumber>;

/// Rialto parachain definition
#[derive(Debug, Clone, Copy)]
pub struct RialtoParachain;

impl ChainBase for RialtoParachain {
	type BlockNumber = rialto_parachain_runtime::BlockNumber;
	type Hash = rialto_parachain_runtime::Hash;
	type Hasher = rialto_parachain_runtime::Hashing;
	type Header = rialto_parachain_runtime::Header;

	type AccountId = rialto_parachain_runtime::AccountId;
	type Balance = rialto_parachain_runtime::Balance;
	type Index = rialto_parachain_runtime::Index;
	type Signature = rialto_parachain_runtime::Signature;

	fn max_extrinsic_size() -> u32 {
		bp_rialto::Rialto::max_extrinsic_size()
	}

	fn max_extrinsic_weight() -> Weight {
		bp_rialto::Rialto::max_extrinsic_weight()
	}
}

impl Chain for RialtoParachain {
	const NAME: &'static str = "RialtoParachain";
	const TOKEN_ID: Option<&'static str> = None;
	// should be fixed/changed in https://github.com/paritytech/parity-bridges-common/pull/1199
	const BEST_FINALIZED_HEADER_ID_METHOD: &'static str = "<UNIMPLEMENTED>";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(5);
	const STORAGE_PROOF_OVERHEAD: u32 = bp_rialto::EXTRA_STORAGE_PROOF_SIZE;
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = bp_rialto::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE;

	type SignedBlock = rialto_parachain_runtime::SignedBlock;
	type Call = rialto_parachain_runtime::Call;
	type WeightToFee = bp_rialto::WeightToFee;
}
