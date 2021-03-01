// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Common types that are used in relay <-> Ethereum node communications.

use headers_relay::sync_types::SourceHeader;

pub use web3::types::{Address, Bytes, CallRequest, SyncState, H256, U128, U256, U64};

/// When header is just received from the Ethereum node, we check that it has
/// both number and hash fields filled.
pub const HEADER_ID_PROOF: &str = "checked on retrieval; qed";

/// Ethereum transaction hash type.
pub type HeaderHash = H256;

/// Ethereum transaction hash type.
pub type TransactionHash = H256;

/// Ethereum transaction type.
pub type Transaction = web3::types::Transaction;

/// Ethereum header type.
pub type Header = web3::types::Block<H256>;

/// Ethereum header type used in headers sync.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncHeader(Header);

impl std::ops::Deref for SyncHeader {
	type Target = Header;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

/// Ethereum header with transactions type.
pub type HeaderWithTransactions = web3::types::Block<Transaction>;

/// Ethereum transaction receipt type.
pub type Receipt = web3::types::TransactionReceipt;

/// Ethereum header ID.
pub type HeaderId = relay_utils::HeaderId<H256, u64>;

/// A raw Ethereum transaction that's been signed.
pub type SignedRawTx = Vec<u8>;

impl From<Header> for SyncHeader {
	fn from(header: Header) -> Self {
		Self(header)
	}
}

impl SourceHeader<H256, u64> for SyncHeader {
	fn id(&self) -> HeaderId {
		relay_utils::HeaderId(
			self.number.expect(HEADER_ID_PROOF).as_u64(),
			self.hash.expect(HEADER_ID_PROOF),
		)
	}

	fn parent_id(&self) -> HeaderId {
		relay_utils::HeaderId(self.number.expect(HEADER_ID_PROOF).as_u64() - 1, self.parent_hash)
	}
}
