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

use polkadot_parachain::primitives::Id as ParaId;
use frame_support::weights::Weight;

pub trait TestExt {
	fn new_ext() -> sp_io::TestExternalities;
	fn reset_ext();
	fn execute_with<R>(execute: impl FnOnce() -> R) -> R;
}

pub trait HandleUmpMessage {
	fn handle_ump_message(from: ParaId, msg: &[u8], max_weight: Weight);
}

pub trait HandleDmpMessage {
	fn handle_dmp_message(at_relay_block: u32, msg: Vec<u8>, max_weight: Weight);
}

pub trait HandleXcmpMessage {
	fn handle_xcmp_message(from: ParaId, at_relay_block: u32, msg: &[u8], max_weight: Weight);
}
