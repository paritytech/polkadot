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

//! Storage keys of bridge token swap pallet.

use frame_support::Identity;
use sp_core::{storage::StorageKey, H256};

/// Name of the `PendingSwaps` storage map.
pub const PENDING_SWAPS_MAP_NAME: &str = "PendingSwaps";

/// Storage key of `PendingSwaps` value with given token swap hash.
pub fn pending_swaps_key(pallet_prefix: &str, token_swap_hash: H256) -> StorageKey {
	bp_runtime::storage_map_final_key::<Identity>(
		pallet_prefix,
		PENDING_SWAPS_MAP_NAME,
		token_swap_hash.as_ref(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use hex_literal::hex;

	#[test]
	fn pending_swaps_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that may break
		// all previous swaps.
		let storage_key = pending_swaps_key("BridgeTokenSwap", [42u8; 32].into()).0;
		assert_eq!(
			storage_key,
			hex!("76276da64e7a4f454760eedeb4bad11adca2227fef56ad07cc424f1f5d128b9a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a").to_vec(),
			"Unexpected storage key: {}",
			hex::encode(&storage_key),
		);
	}
}
