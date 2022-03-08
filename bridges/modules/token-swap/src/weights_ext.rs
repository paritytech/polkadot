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

//! Weight-related utilities.

use crate::weights::WeightInfo;

use bp_runtime::Size;
use frame_support::weights::{RuntimeDbWeight, Weight};

/// Extended weight info.
pub trait WeightInfoExt: WeightInfo {
	// Functions that are directly mapped to extrinsics weights.

	/// Weight of message send extrinsic.
	fn send_message_weight(message: &impl Size, db_weight: RuntimeDbWeight) -> Weight;
}

impl WeightInfoExt for () {
	fn send_message_weight(message: &impl Size, db_weight: RuntimeDbWeight) -> Weight {
		<() as pallet_bridge_messages::WeightInfoExt>::send_message_weight(message, db_weight)
	}
}

impl<T: frame_system::Config> WeightInfoExt for crate::weights::MillauWeight<T> {
	fn send_message_weight(message: &impl Size, db_weight: RuntimeDbWeight) -> Weight {
		<() as pallet_bridge_messages::WeightInfoExt>::send_message_weight(message, db_weight)
	}
}
