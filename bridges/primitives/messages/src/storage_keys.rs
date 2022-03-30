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

//! Storage keys of bridge messages pallet.

/// Name of the `OPERATING_MODE_VALUE_NAME` storage value.
pub const OPERATING_MODE_VALUE_NAME: &str = "PalletOperatingMode";
/// Name of the `OutboundMessages` storage map.
pub const OUTBOUND_MESSAGES_MAP_NAME: &str = "OutboundMessages";
/// Name of the `OutboundLanes` storage map.
pub const OUTBOUND_LANES_MAP_NAME: &str = "OutboundLanes";
/// Name of the `InboundLanes` storage map.
pub const INBOUND_LANES_MAP_NAME: &str = "InboundLanes";

use crate::{LaneId, MessageKey, MessageNonce};

use codec::Encode;
use frame_support::Blake2_128Concat;
use sp_core::storage::StorageKey;

/// Storage key of the `PalletOperatingMode` value in the runtime storage.
pub fn operating_mode_key(pallet_prefix: &str) -> StorageKey {
	StorageKey(
		bp_runtime::storage_value_final_key(
			pallet_prefix.as_bytes(),
			OPERATING_MODE_VALUE_NAME.as_bytes(),
		)
		.to_vec(),
	)
}

/// Storage key of the outbound message in the runtime storage.
pub fn message_key(pallet_prefix: &str, lane: &LaneId, nonce: MessageNonce) -> StorageKey {
	bp_runtime::storage_map_final_key::<Blake2_128Concat>(
		pallet_prefix,
		OUTBOUND_MESSAGES_MAP_NAME,
		&MessageKey { lane_id: *lane, nonce }.encode(),
	)
}

/// Storage key of the outbound message lane state in the runtime storage.
pub fn outbound_lane_data_key(pallet_prefix: &str, lane: &LaneId) -> StorageKey {
	bp_runtime::storage_map_final_key::<Blake2_128Concat>(
		pallet_prefix,
		OUTBOUND_LANES_MAP_NAME,
		lane,
	)
}

/// Storage key of the inbound message lane state in the runtime storage.
pub fn inbound_lane_data_key(pallet_prefix: &str, lane: &LaneId) -> StorageKey {
	bp_runtime::storage_map_final_key::<Blake2_128Concat>(
		pallet_prefix,
		INBOUND_LANES_MAP_NAME,
		lane,
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use hex_literal::hex;

	#[test]
	fn operating_mode_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is possibly
		// breaking all existing message relays.
		let storage_key = operating_mode_key("BridgeMessages").0;
		assert_eq!(
			storage_key,
			hex!("dd16c784ebd3390a9bc0357c7511ed010f4cf0917788d791142ff6c1f216e7b3").to_vec(),
			"Unexpected storage key: {}",
			hex::encode(&storage_key),
		);
	}

	#[test]
	fn storage_message_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is breaking
		// all previously crafted messages proofs.
		let storage_key = message_key("BridgeMessages", &*b"test", 42).0;
		assert_eq!(
			storage_key,
			hex!("dd16c784ebd3390a9bc0357c7511ed018a395e6242c6813b196ca31ed0547ea79446af0e09063bd4a7874aef8a997cec746573742a00000000000000").to_vec(),
			"Unexpected storage key: {}",
			hex::encode(&storage_key),
		);
	}

	#[test]
	fn outbound_lane_data_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is breaking
		// all previously crafted outbound lane state proofs.
		let storage_key = outbound_lane_data_key("BridgeMessages", &*b"test").0;
		assert_eq!(
			storage_key,
			hex!("dd16c784ebd3390a9bc0357c7511ed0196c246acb9b55077390e3ca723a0ca1f44a8995dd50b6657a037a7839304535b74657374").to_vec(),
			"Unexpected storage key: {}",
			hex::encode(&storage_key),
		);
	}

	#[test]
	fn inbound_lane_data_key_computed_properly() {
		// If this test fails, then something has been changed in module storage that is breaking
		// all previously crafted inbound lane state proofs.
		let storage_key = inbound_lane_data_key("BridgeMessages", &*b"test").0;
		assert_eq!(
			storage_key,
			hex!("dd16c784ebd3390a9bc0357c7511ed01e5f83cf83f2127eb47afdc35d6e43fab44a8995dd50b6657a037a7839304535b74657374").to_vec(),
			"Unexpected storage key: {}",
			hex::encode(&storage_key),
		);
	}
}
