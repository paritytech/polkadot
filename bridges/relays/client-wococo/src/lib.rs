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

//! Types used to connect to the Wococo-Substrate chain.

use bp_messages::MessageNonce;
use codec::Encode;
use frame_support::weights::Weight;
use relay_substrate_client::{
	Chain, ChainBase, ChainWithBalances, ChainWithGrandpa, ChainWithMessages,
	Error as SubstrateError, SignParam, TransactionSignScheme, UnsignedTransaction,
};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

pub mod runtime;

/// Wococo header id.
pub type HeaderId = relay_utils::HeaderId<bp_wococo::Hash, bp_wococo::BlockNumber>;

/// Wococo header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_wococo::Header>;

/// Wococo chain definition
#[derive(Debug, Clone, Copy)]
pub struct Wococo;

impl ChainBase for Wococo {
	type BlockNumber = bp_wococo::BlockNumber;
	type Hash = bp_wococo::Hash;
	type Hasher = bp_wococo::Hashing;
	type Header = bp_wococo::Header;

	type AccountId = bp_wococo::AccountId;
	type Balance = bp_wococo::Balance;
	type Index = bp_wococo::Nonce;
	type Signature = bp_wococo::Signature;

	fn max_extrinsic_size() -> u32 {
		bp_wococo::Wococo::max_extrinsic_size()
	}

	fn max_extrinsic_weight() -> Weight {
		bp_wococo::Wococo::max_extrinsic_weight()
	}
}

impl Chain for Wococo {
	const NAME: &'static str = "Wococo";
	const TOKEN_ID: Option<&'static str> = None;
	const BEST_FINALIZED_HEADER_ID_METHOD: &'static str =
		bp_wococo::BEST_FINALIZED_WOCOCO_HEADER_METHOD;
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);
	const STORAGE_PROOF_OVERHEAD: u32 = bp_wococo::EXTRA_STORAGE_PROOF_SIZE;
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = bp_wococo::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE;

	type SignedBlock = bp_wococo::SignedBlock;
	type Call = crate::runtime::Call;
	type WeightToFee = bp_wococo::WeightToFee;
}

impl ChainWithGrandpa for Wococo {
	const WITH_CHAIN_GRANDPA_PALLET_NAME: &'static str = bp_wococo::WITH_WOCOCO_GRANDPA_PALLET_NAME;
}

impl ChainWithMessages for Wococo {
	const WITH_CHAIN_MESSAGES_PALLET_NAME: &'static str =
		bp_wococo::WITH_WOCOCO_MESSAGES_PALLET_NAME;
	const TO_CHAIN_MESSAGE_DETAILS_METHOD: &'static str =
		bp_wococo::TO_WOCOCO_MESSAGE_DETAILS_METHOD;
	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_CHAIN: Weight =
		bp_wococo::PAY_INBOUND_DISPATCH_FEE_WEIGHT;
	const MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX: MessageNonce =
		bp_wococo::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX;
	const MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX: MessageNonce =
		bp_wococo::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX;
	type WeightInfo = ();
}

impl ChainWithBalances for Wococo {
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		StorageKey(bp_wococo::account_info_storage_key(account_id))
	}
}

impl TransactionSignScheme for Wococo {
	type Chain = Wococo;
	type AccountKeyPair = sp_core::sr25519::Pair;
	type SignedTransaction = crate::runtime::UncheckedExtrinsic;

	fn sign_transaction(param: SignParam<Self>) -> Result<Self::SignedTransaction, SubstrateError> {
		let raw_payload = SignedPayload::new(
			param.unsigned.call.clone(),
			bp_wococo::SignedExtensions::new(
				param.spec_version,
				param.transaction_version,
				param.era,
				param.genesis_hash,
				param.unsigned.nonce,
				param.unsigned.tip,
			),
		)
		.expect("SignedExtension never fails.");

		let signature = raw_payload.using_encoded(|payload| param.signer.sign(payload));
		let signer: sp_runtime::MultiSigner = param.signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		Ok(bp_wococo::UncheckedExtrinsic::new_signed(
			call,
			sp_runtime::MultiAddress::Id(signer.into_account()),
			signature.into(),
			extra,
		))
	}

	fn is_signed(tx: &Self::SignedTransaction) -> bool {
		tx.signature.is_some()
	}

	fn is_signed_by(signer: &Self::AccountKeyPair, tx: &Self::SignedTransaction) -> bool {
		tx.signature
			.as_ref()
			.map(|(address, _, _)| {
				*address == bp_wococo::AccountId::from(*signer.public().as_array_ref()).into()
			})
			.unwrap_or(false)
	}

	fn parse_transaction(tx: Self::SignedTransaction) -> Option<UnsignedTransaction<Self::Chain>> {
		let extra = &tx.signature.as_ref()?.2;
		Some(UnsignedTransaction { call: tx.function, nonce: extra.nonce(), tip: extra.tip() })
	}
}

/// Wococo signing params.
pub type SigningParams = sp_core::sr25519::Pair;
