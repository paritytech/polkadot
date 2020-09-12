// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Cross-Consensus Message format data structures.

use sp_std::vec::Vec;
use sp_runtime::RuntimeDebug;
use codec::{self, Encode, Decode};

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum NetworkId {
	/// Unidentified/any.
	Any,
	/// Some named network.
	Named(Vec<u8>),
	/// The Polkadot Relay chain
	Polkadot,
	/// Kusama.
	Kusama,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum Junction {
	Parent,
	Parachain { #[codec(compact)] id: u32 },
	OpaqueRemark(Vec<u8>),
	AccountId32 { network: NetworkId, id: [u8; 32] },
	AccountIndex64 { network: NetworkId, #[codec(compact)] index: u64 },
	AccountKey20 { network: NetworkId, key: [u8; 20] },
	/// An instanced Pallet on a Frame-based chain.
	PalletInstance { id: u8 },
	/// A nondescript index within the context location.
	GeneralIndex { #[codec(compact)] id: u128 },
	/// A nondescript datum acting as a key within the context location.
	GeneralKey(Vec<u8>),
	/// The unambiguous child. Not currently used except as a fallback when deriving ancestry.
	OnlyChild,
}

impl Junction {
	pub fn is_sub_consensus(&self) -> bool {
		match self {
			Junction::Parent => false,

			Junction::Parachain { .. } |
			Junction::OpaqueRemark(..) |
			Junction::AccountId32 { .. } |
			Junction::AccountIndex64 { .. } |
			Junction::AccountKey20 { .. } |
			Junction::PalletInstance { .. } |
			Junction::GeneralIndex { .. } |
			Junction::GeneralKey(..) |
			Junction::OnlyChild => true,
		}
	}
}
