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

use bp_messages::MessageNonce;
use bp_runtime::{Chain as ChainBase, EncodedOrDecodedCall, HashOf, TransactionEraOf};
use codec::{Codec, Encode};
use frame_support::weights::{Weight, WeightToFeePolynomial};
use jsonrpsee::core::{DeserializeOwned, Serialize};
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
	/// Identifier of the basic token of the chain (if applicable).
	///
	/// This identifier is used to fetch token price. In case of testnets, you may either
	/// set it to `None`, or associate testnet with one of the existing tokens.
	const TOKEN_ID: Option<&'static str>;
	/// Name of the runtime API method that is returning best known finalized header number
	/// and hash (as tuple).
	///
	/// Keep in mind that this method is normally provided by the other chain, which is
	/// bridged with this chain.
	const BEST_FINALIZED_HEADER_ID_METHOD: &'static str;

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
	type Call: Clone + Codec + Dispatchable + Debug + Send;

	/// Type that is used by the chain, to convert from weight to fee.
	type WeightToFee: WeightToFeePolynomial<Balance = Self::Balance>;
}

/// Substrate-based chain that is using direct GRANDPA finality from minimal relay-client point of
/// view.
///
/// Keep in mind that parachains are relying on relay chain GRANDPA, so they should not implement
/// this trait.
pub trait ChainWithGrandpa: Chain {
	/// Name of the bridge GRANDPA pallet (used in `construct_runtime` macro call) that is deployed
	/// at some other chain to bridge with this `ChainWithGrandpa`.
	///
	/// We assume that all chains that are bridging with this `ChainWithGrandpa` are using
	/// the same name.
	const WITH_CHAIN_GRANDPA_PALLET_NAME: &'static str;
}

/// Substrate-based chain with messaging support from minimal relay-client point of view.
pub trait ChainWithMessages: Chain {
	/// Name of the bridge messages pallet (used in `construct_runtime` macro call) that is deployed
	/// at some other chain to bridge with this `ChainWithMessages`.
	///
	/// We assume that all chains that are bridging with this `ChainWithMessages` are using
	/// the same name.
	const WITH_CHAIN_MESSAGES_PALLET_NAME: &'static str;

	/// Name of the `To<ChainWithMessages>OutboundLaneApi::message_details` runtime API method.
	/// The method is provided by the runtime that is bridged with this `ChainWithMessages`.
	const TO_CHAIN_MESSAGE_DETAILS_METHOD: &'static str;

	/// Additional weight of the dispatch fee payment if dispatch is paid at the target chain
	/// and this `ChainWithMessages` is the target chain.
	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_CHAIN: Weight;

	/// Maximal number of unrewarded relayers in a single confirmation transaction at this
	/// `ChainWithMessages`.
	const MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX: MessageNonce;
	/// Maximal number of unconfirmed messages in a single confirmation transaction at this
	/// `ChainWithMessages`.
	const MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX: MessageNonce;

	/// Weights of message pallet calls.
	type WeightInfo: pallet_bridge_messages::WeightInfoExt;
}

/// Call type used by the chain.
pub type CallOf<C> = <C as Chain>::Call;
/// Weight-to-Fee type used by the chain.
pub type WeightToFeeOf<C> = <C as Chain>::WeightToFee;
/// Transaction status of the chain.
pub type TransactionStatusOf<C> = TransactionStatus<HashOf<C>, HashOf<C>>;

/// Substrate-based chain with `AccountData` generic argument of `frame_system::AccountInfo` set to
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
#[derive(Clone, Debug, PartialEq)]
pub struct UnsignedTransaction<C: Chain> {
	/// Runtime call of this transaction.
	pub call: EncodedOrDecodedCall<C::Call>,
	/// Transaction nonce.
	pub nonce: C::Index,
	/// Tip included into transaction.
	pub tip: C::Balance,
}

impl<C: Chain> UnsignedTransaction<C> {
	/// Create new unsigned transaction with given call, nonce and zero tip.
	pub fn new(call: EncodedOrDecodedCall<C::Call>, nonce: C::Index) -> Self {
		Self { call, nonce, tip: Zero::zero() }
	}

	/// Set transaction tip.
	pub fn tip(mut self, tip: C::Balance) -> Self {
		self.tip = tip;
		self
	}
}

/// Account key pair used by transactions signing scheme.
pub type AccountKeyPairOf<S> = <S as TransactionSignScheme>::AccountKeyPair;

/// Substrate-based chain transactions signing scheme.
pub trait TransactionSignScheme {
	/// Chain that this scheme is to be used.
	type Chain: Chain;
	/// Type of key pairs used to sign transactions.
	type AccountKeyPair: Pair;
	/// Signed transaction.
	type SignedTransaction: Clone + Debug + Codec + Send + 'static;

	/// Create transaction for given runtime call, signed by given account.
	fn sign_transaction(param: SignParam<Self>) -> Result<Self::SignedTransaction, crate::Error>
	where
		Self: Sized;

	/// Returns true if transaction is signed.
	fn is_signed(tx: &Self::SignedTransaction) -> bool;

	/// Returns true if transaction is signed by given signer.
	fn is_signed_by(signer: &Self::AccountKeyPair, tx: &Self::SignedTransaction) -> bool;

	/// Parse signed transaction into its unsigned part.
	///
	/// Returns `None` if signed transaction has unsupported format.
	fn parse_transaction(tx: Self::SignedTransaction) -> Option<UnsignedTransaction<Self::Chain>>;
}

/// Sign transaction parameters
pub struct SignParam<T: TransactionSignScheme> {
	/// Version of the runtime specification.
	pub spec_version: u32,
	/// Transaction version
	pub transaction_version: u32,
	/// Hash of the genesis block.
	pub genesis_hash: <T::Chain as ChainBase>::Hash,
	/// Signer account
	pub signer: T::AccountKeyPair,
	/// Transaction era used by the chain.
	pub era: TransactionEraOf<T::Chain>,
	/// Transaction before it is signed.
	pub unsigned: UnsignedTransaction<T::Chain>,
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
