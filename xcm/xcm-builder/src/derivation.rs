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

use parity_scale_codec::{Decode, Encode};
use sp_core::H256;
use sp_io::hashing::blake2_256;
use sp_runtime::traits::TrailingZeroInput;

/// Constant derivation function for Tinkernet Multisigs.
/// Uses the Tinkernet genesis hash as a salt.
pub fn derive_tinkernet_multisig<AccountId: Decode>(
	id: u128,
) -> Result<AccountId, <u32 as TryFrom<u128>>::Error> {
	Ok(AccountId::decode(&mut TrailingZeroInput::new(
		&(
			// The constant salt used to derive Tinkernet Multisigs, this is Tinkernet's genesis hash.
			H256([
				212, 46, 150, 6, 169, 149, 223, 228, 51, 220, 121, 85, 220, 42, 112, 244, 149, 243,
				80, 243, 115, 218, 162, 0, 9, 138, 232, 68, 55, 129, 106, 210,
			]),
			// The actual multisig integer id.
			u32::try_from(id)?,
		)
			.using_encoded(blake2_256),
	))
	.expect("infinite length input; no invalid inputs for type; qed"))
}
