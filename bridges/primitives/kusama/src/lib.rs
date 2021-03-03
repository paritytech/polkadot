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

#![cfg_attr(not(feature = "std"), no_std)]
// RuntimeApi generated functions
#![allow(clippy::too_many_arguments)]
// Runtime-generated DecodeLimit::decode_all_with_depth_limit
#![allow(clippy::unnecessary_mut_passed)]

use bp_message_lane::{LaneId, MessageNonce};
use bp_runtime::Chain;
use frame_support::{weights::Weight, RuntimeDebug};
use sp_core::Hasher as HasherT;
use sp_runtime::{
	generic,
	traits::{BlakeTwo256, IdentifyAccount, Verify},
	MultiSignature, OpaqueExtrinsic as UncheckedExtrinsic,
};
use sp_std::prelude::*;

// TODO: may need to be updated after https://github.com/paritytech/parity-bridges-common/issues/78
/// Maximal number of messages in single delivery transaction.
pub const MAX_MESSAGES_IN_DELIVERY_TRANSACTION: MessageNonce = 128;

/// Maximal number of unrewarded relayer entries at inbound lane.
pub const MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE: MessageNonce = 128;

// TODO: should be selected keeping in mind:
// finality delay on both chains + reward payout cost + messages throughput.
/// Maximal number of unconfirmed messages at inbound lane.
pub const MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE: MessageNonce = 8192;

/// Block number type used in Kusama.
pub type BlockNumber = u32;

/// Hash type used in Kusama.
pub type Hash = <BlakeTwo256 as HasherT>::Out;

/// The type of an object that can produce hashes on Kusama.
pub type Hasher = BlakeTwo256;

/// The header type used by Kusama.
pub type Header = generic::Header<BlockNumber, Hasher>;

/// Signature type used by Kusama.
pub type Signature = MultiSignature;

/// Public key of account on Kusama chain.
pub type AccountPublic = <Signature as Verify>::Signer;

/// Id of account on Kusama chain.
pub type AccountId = <AccountPublic as IdentifyAccount>::AccountId;

/// Index of a transaction on the Kusama chain.
pub type Nonce = u32;

/// Block type of Kusama.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// Kusama block signed with a Justification.
pub type SignedBlock = generic::SignedBlock<Block>;

/// The balance of an account on Polkadot.
pub type Balance = u128;

/// Kusama chain.
#[derive(RuntimeDebug)]
pub struct Kusama;

impl Chain for Kusama {
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hasher = Hasher;
	type Header = Header;
}

/// Convert a 256-bit hash into an AccountId.
pub struct AccountIdConverter;

impl sp_runtime::traits::Convert<sp_core::H256, AccountId> for AccountIdConverter {
	fn convert(hash: sp_core::H256) -> AccountId {
		hash.to_fixed_bytes().into()
	}
}

/// Name of the `KusamaHeaderApi::best_blocks` runtime method.
pub const BEST_KUSAMA_BLOCKS_METHOD: &str = "KusamaHeaderApi_best_blocks";
/// Name of the `KusamaHeaderApi::finalized_block` runtime method.
pub const FINALIZED_KUSAMA_BLOCK_METHOD: &str = "KusamaHeaderApi_finalized_block";
/// Name of the `KusamaHeaderApi::is_known_block` runtime method.
pub const IS_KNOWN_KUSAMA_BLOCK_METHOD: &str = "KusamaHeaderApi_is_known_block";
/// Name of the `KusamaHeaderApi::incomplete_headers` runtime method.
pub const INCOMPLETE_KUSAMA_HEADERS_METHOD: &str = "KusamaHeaderApi_incomplete_headers";

sp_api::decl_runtime_apis! {
	/// API for querying information about Kusama headers from the Bridge Pallet instance.
	///
	/// This API is implemented by runtimes that are bridging with Kusama chain, not the
	/// Kusama runtime itself.
	pub trait KusamaHeaderApi {
		/// Returns number and hash of the best blocks known to the bridge module.
		///
		/// Will return multiple headers if there are many headers at the same "best" height.
		///
		/// The caller should only submit an `import_header` transaction that makes
		/// (or leads to making) other header the best one.
		fn best_blocks() -> Vec<(BlockNumber, Hash)>;
		/// Returns number and hash of the best finalized block known to the bridge module.
		fn finalized_block() -> (BlockNumber, Hash);
		/// Returns numbers and hashes of headers that require finality proofs.
		///
		/// An empty response means that there are no headers which currently require a
		/// finality proof.
		fn incomplete_headers() -> Vec<(BlockNumber, Hash)>;
		/// Returns true if the header is known to the runtime.
		fn is_known_block(hash: Hash) -> bool;
		/// Returns true if the header is considered finalized by the runtime.
		fn is_finalized_block(hash: Hash) -> bool;
	}

	/// Outbound message lane API for messages that are sent to Kusama chain.
	///
	/// This API is implemented by runtimes that are sending messages to Kusama chain, not the
	/// Kusama runtime itself.
	pub trait ToKusamaOutboundLaneApi {
		/// Returns total dispatch weight and encoded payload size of all messages in given inclusive range.
		///
		/// If some (or all) messages are missing from the storage, they'll also will
		/// be missing from the resulting vector. The vector is ordered by the nonce.
		fn messages_dispatch_weight(
			lane: LaneId,
			begin: MessageNonce,
			end: MessageNonce,
		) -> Vec<(MessageNonce, Weight, u32)>;
		/// Returns nonce of the latest message, received by bridged chain.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Returns nonce of the latest message, generated by given lane.
		fn latest_generated_nonce(lane: LaneId) -> MessageNonce;
	}

	/// Inbound message lane API for messages sent by Kusama chain.
	///
	/// This API is implemented by runtimes that are receiving messages from Kusama chain, not the
	/// Kusama runtime itself.
	pub trait FromKusamaInboundLaneApi {
		/// Returns nonce of the latest message, received by given lane.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Nonce of latest message that has been confirmed to the bridged chain.
		fn latest_confirmed_nonce(lane: LaneId) -> MessageNonce;
	}
}
