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

//! Types used to connect to the Westend chain.

use frame_support::weights::Weight;
use relay_substrate_client::{Chain, ChainBase, ChainWithBalances, ChainWithGrandpa};
use sp_core::storage::StorageKey;
use std::time::Duration;

/// Westend header id.
pub type HeaderId = relay_utils::HeaderId<bp_westend::Hash, bp_westend::BlockNumber>;

/// Westend header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_westend::Header>;

/// Westend chain definition
#[derive(Debug, Clone, Copy)]
pub struct Westend;

impl ChainBase for Westend {
	type BlockNumber = bp_westend::BlockNumber;
	type Hash = bp_westend::Hash;
	type Hasher = bp_westend::Hasher;
	type Header = bp_westend::Header;

	type AccountId = bp_westend::AccountId;
	type Balance = bp_westend::Balance;
	type Index = bp_westend::Nonce;
	type Signature = bp_westend::Signature;

	fn max_extrinsic_size() -> u32 {
		bp_westend::Westend::max_extrinsic_size()
	}

	fn max_extrinsic_weight() -> Weight {
		bp_westend::Westend::max_extrinsic_weight()
	}
}

impl Chain for Westend {
	const NAME: &'static str = "Westend";
	const TOKEN_ID: Option<&'static str> = None;
	const BEST_FINALIZED_HEADER_ID_METHOD: &'static str =
		bp_westend::BEST_FINALIZED_WESTEND_HEADER_METHOD;
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);
	const STORAGE_PROOF_OVERHEAD: u32 = bp_westend::EXTRA_STORAGE_PROOF_SIZE;
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = bp_westend::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE;

	type SignedBlock = bp_westend::SignedBlock;
	type Call = bp_westend::Call;
	type WeightToFee = bp_westend::WeightToFee;
}

impl ChainWithGrandpa for Westend {
	const WITH_CHAIN_GRANDPA_PALLET_NAME: &'static str =
		bp_westend::WITH_WESTEND_GRANDPA_PALLET_NAME;
}

impl ChainWithBalances for Westend {
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		StorageKey(bp_westend::account_info_storage_key(account_id))
	}
}
