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

use bp_runtime::Chain as ChainBase;
use frame_support::Parameter;
use jsonrpsee_ws_client::{DeserializeOwned, Serialize};
use num_traits::{CheckedSub, Zero};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{
	generic::SignedBlock,
	traits::{
		AtLeast32Bit, Block as BlockT, Dispatchable, MaybeDisplay, MaybeSerialize, MaybeSerializeDeserialize, Member,
	},
	EncodedJustification,
};
use std::{fmt::Debug, time::Duration};

/// Substrate-based chain from minimal relay-client point of view.
pub trait Chain: ChainBase + Clone {
	/// Chain name.
	const NAME: &'static str;
	/// Average block interval.
	///
	/// How often blocks are produced on that chain. It's suggested to set this value
	/// to match the block time of the chain.
	const AVERAGE_BLOCK_INTERVAL: Duration;

	/// The user account identifier type for the runtime.
	type AccountId: Parameter + Member + MaybeSerializeDeserialize + Debug + MaybeDisplay + Ord + Default;
	/// Index of a transaction used by the chain.
	type Index: Parameter
		+ Member
		+ MaybeSerialize
		+ Debug
		+ Default
		+ MaybeDisplay
		+ DeserializeOwned
		+ AtLeast32Bit
		+ Copy;
	/// Block type.
	type SignedBlock: Member + Serialize + DeserializeOwned + BlockWithJustification<Self::Header>;
	/// The aggregated `Call` type.
	type Call: Dispatchable + Debug;
}

/// Substrate-based chain with `frame_system::Config::AccountData` set to
/// the `pallet_balances::AccountData<NativeBalance>`.
pub trait ChainWithBalances: Chain {
	/// Balance of an account in native tokens.
	type NativeBalance: Parameter + Member + DeserializeOwned + Clone + Copy + CheckedSub + PartialOrd + Zero;

	/// Return runtime storage key for getting `frame_system::AccountInfo` of given account.
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey;
}

/// Block with justification.
pub trait BlockWithJustification<Header> {
	/// Return block header.
	fn header(&self) -> Header;
	/// Return block justification, if known.
	fn justification(&self) -> Option<&EncodedJustification>;
}

/// Substrate-based chain transactions signing scheme.
pub trait TransactionSignScheme {
	/// Chain that this scheme is to be used.
	type Chain: Chain;
	/// Type of key pairs used to sign transactions.
	type AccountKeyPair: Pair;
	/// Signed transaction.
	type SignedTransaction;

	/// Create transaction for given runtime call, signed by given account.
	fn sign_transaction(
		genesis_hash: <Self::Chain as ChainBase>::Hash,
		signer: &Self::AccountKeyPair,
		signer_nonce: <Self::Chain as Chain>::Index,
		call: <Self::Chain as Chain>::Call,
	) -> Self::SignedTransaction;
}

impl<Block: BlockT> BlockWithJustification<Block::Header> for SignedBlock<Block> {
	fn header(&self) -> Block::Header {
		self.block.header().clone()
	}

	fn justification(&self) -> Option<&EncodedJustification> {
		self.justifications
			.as_ref()
			.and_then(|j| j.get(sp_finality_grandpa::GRANDPA_ENGINE_ID))
	}
}
