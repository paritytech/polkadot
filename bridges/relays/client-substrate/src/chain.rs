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

use bp_runtime::{Chain as ChainBase, HashOf, TransactionEraOf};
use codec::{Codec, Encode};
use frame_support::weights::WeightToFeePolynomial;
use jsonrpsee_ws_client::types::{DeserializeOwned, Serialize};
use num_traits::Zero;
use sc_transaction_pool_api::TransactionStatus;
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{
	generic::SignedBlock,
	traits::{Block as BlockT, Dispatchable, Member},
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
	/// Maximal expected storage proof overhead (in bytes).
	const STORAGE_PROOF_OVERHEAD: u32;
	/// Maximal size (in bytes) of SCALE-encoded account id on this chain.
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32;

	/// Block type.
	type SignedBlock: Member + Serialize + DeserializeOwned + BlockWithJustification<Self::Header>;
	/// The aggregated `Call` type.
	type Call: Clone + Dispatchable + Debug;

	/// Type that is used by the chain, to convert from weight to fee.
	type WeightToFee: WeightToFeePolynomial<Balance = Self::Balance>;
}

/// Call type used by the chain.
pub type CallOf<C> = <C as Chain>::Call;
/// Weight-to-Fee type used by the chain.
pub type WeightToFeeOf<C> = <C as Chain>::WeightToFee;
/// Transaction status of the chain.
pub type TransactionStatusOf<C> = TransactionStatus<HashOf<C>, HashOf<C>>;

/// Substrate-based chain with `frame_system::Config::AccountData` set to
/// the `pallet_balances::AccountData<Balance>`.
pub trait ChainWithBalances: Chain {
	/// Return runtime storage key for getting `frame_system::AccountInfo` of given account.
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey;
}

/// SCALE-encoded extrinsic.
pub type EncodedExtrinsic = Vec<u8>;

/// Block with justification.
pub trait BlockWithJustification<Header> {
	/// Return block header.
	fn header(&self) -> Header;
	/// Return encoded block extrinsics.
	fn extrinsics(&self) -> Vec<EncodedExtrinsic>;
	/// Return block justification, if known.
	fn justification(&self) -> Option<&EncodedJustification>;
}

/// Transaction before it is signed.
#[derive(Clone, Debug)]
pub struct UnsignedTransaction<C: Chain> {
	/// Runtime call of this transaction.
	pub call: C::Call,
	/// Transaction nonce.
	pub nonce: C::Index,
	/// Tip included into transaction.
	pub tip: C::Balance,
}

impl<C: Chain> UnsignedTransaction<C> {
	/// Create new unsigned transaction with given call, nonce and zero tip.
	pub fn new(call: C::Call, nonce: C::Index) -> Self {
		Self { call, nonce, tip: Zero::zero() }
	}

	/// Set transaction tip.
	pub fn tip(mut self, tip: C::Balance) -> Self {
		self.tip = tip;
		self
	}
}

/// Substrate-based chain transactions signing scheme.
pub trait TransactionSignScheme {
	/// Chain that this scheme is to be used.
	type Chain: Chain;
	/// Type of key pairs used to sign transactions.
	type AccountKeyPair: Pair;
	/// Signed transaction.
	type SignedTransaction: Clone + Debug + Codec + Send + 'static;

	/// Create transaction for given runtime call, signed by given account.
	fn sign_transaction(
		genesis_hash: <Self::Chain as ChainBase>::Hash,
		signer: &Self::AccountKeyPair,
		era: TransactionEraOf<Self::Chain>,
		unsigned: UnsignedTransaction<Self::Chain>,
	) -> Self::SignedTransaction;

	/// Returns true if transaction is signed.
	fn is_signed(tx: &Self::SignedTransaction) -> bool;

	/// Returns true if transaction is signed by given signer.
	fn is_signed_by(signer: &Self::AccountKeyPair, tx: &Self::SignedTransaction) -> bool;

	/// Parse signed transaction into its unsigned part.
	///
	/// Returns `None` if signed transaction has unsupported format.
	fn parse_transaction(tx: Self::SignedTransaction) -> Option<UnsignedTransaction<Self::Chain>>;
}

impl<Block: BlockT> BlockWithJustification<Block::Header> for SignedBlock<Block> {
	fn header(&self) -> Block::Header {
		self.block.header().clone()
	}

	fn extrinsics(&self) -> Vec<EncodedExtrinsic> {
		self.block.extrinsics().iter().map(Encode::encode).collect()
	}

	fn justification(&self) -> Option<&EncodedJustification> {
		self.justifications
			.as_ref()
			.and_then(|j| j.get(sp_finality_grandpa::GRANDPA_ENGINE_ID))
	}
}
