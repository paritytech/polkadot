// Copyright (C) Parity Technologies (UK) Ltd.
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

use xcm::prelude::*;

/// Handle stuff to do with taking fees in certain XCM instructions.
pub trait FeeManager {
	/// Determine if a fee which would normally payable should be waived.
	fn is_waived(origin: Option<&MultiLocation>, r: FeeReason) -> bool;

	/// Do something with the fee which has been paid. Doing nothing here silently burns the
	/// fees.
	fn handle_fee(fee: MultiAssets);
}

/// Context under which a fee is paid.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum FeeReason {
	/// When a reporting instruction is called.
	Report,
	/// When the `TransferReserveAsset` instruction is called.
	TransferReserveAsset,
	/// When the `DepositReserveAsset` instruction is called.
	DepositReserveAsset,
	/// When the `InitiateReserveWithdraw` instruction is called.
	InitiateReserveWithdraw,
	/// When the `InitiateTeleport` instruction is called.
	InitiateTeleport,
	/// When the `QueryPallet` instruction is called.
	QueryPallet,
	/// When the `ExportMessage` instruction is called (and includes the network ID).
	Export(NetworkId),
	/// The `charge_fees` API.
	ChargeFees,
	/// When the `LockAsset` instruction is called.
	LockAsset,
	/// When the `RequestUnlock` instruction is called.
	RequestUnlock,
}

impl FeeManager for () {
	fn is_waived(_: Option<&MultiLocation>, _: FeeReason) -> bool {
		true
	}
	fn handle_fee(_: MultiAssets) {}
}
