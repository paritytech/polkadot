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

//! Types used to connect to the Millau-Substrate chain.

use bp_messages::MessageNonce;
use codec::{Compact, Decode, Encode};
use frame_support::weights::Weight;
use relay_substrate_client::{
	BalanceOf, Chain, ChainBase, ChainWithBalances, ChainWithGrandpa, ChainWithMessages,
	Error as SubstrateError, IndexOf, SignParam, TransactionSignScheme, UnsignedTransaction,
};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

/// Millau header id.
pub type HeaderId = relay_utils::HeaderId<millau_runtime::Hash, millau_runtime::BlockNumber>;

/// Millau chain definition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Millau;

impl ChainBase for Millau {
	type BlockNumber = millau_runtime::BlockNumber;
	type Hash = millau_runtime::Hash;
	type Hasher = millau_runtime::Hashing;
	type Header = millau_runtime::Header;

	type AccountId = millau_runtime::AccountId;
	type Balance = millau_runtime::Balance;
	type Index = millau_runtime::Index;
	type Signature = millau_runtime::Signature;

	fn max_extrinsic_size() -> u32 {
		bp_millau::Millau::max_extrinsic_size()
	}

	fn max_extrinsic_weight() -> Weight {
		bp_millau::Millau::max_extrinsic_weight()
	}
}

impl ChainWithGrandpa for Millau {
	const WITH_CHAIN_GRANDPA_PALLET_NAME: &'static str = bp_millau::WITH_MILLAU_GRANDPA_PALLET_NAME;
}

impl ChainWithMessages for Millau {
	const WITH_CHAIN_MESSAGES_PALLET_NAME: &'static str =
		bp_millau::WITH_MILLAU_MESSAGES_PALLET_NAME;
	const TO_CHAIN_MESSAGE_DETAILS_METHOD: &'static str =
		bp_millau::TO_MILLAU_MESSAGE_DETAILS_METHOD;
	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_CHAIN: Weight =
		bp_millau::PAY_INBOUND_DISPATCH_FEE_WEIGHT;
	const MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX: MessageNonce =
		bp_millau::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX;
	const MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX: MessageNonce =
		bp_millau::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX;
	type WeightInfo = ();
}

impl Chain for Millau {
	const NAME: &'static str = "Millau";
	// Rialto token has no value, but we associate it with KSM token
	const TOKEN_ID: Option<&'static str> = Some("kusama");
	const BEST_FINALIZED_HEADER_ID_METHOD: &'static str =
		bp_millau::BEST_FINALIZED_MILLAU_HEADER_METHOD;
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(5);
	const STORAGE_PROOF_OVERHEAD: u32 = bp_millau::EXTRA_STORAGE_PROOF_SIZE;
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = bp_millau::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE;

	type SignedBlock = millau_runtime::SignedBlock;
	type Call = millau_runtime::Call;
	type WeightToFee = bp_millau::WeightToFee;
}

impl ChainWithBalances for Millau {
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		use frame_support::storage::generator::StorageMap;
		StorageKey(frame_system::Account::<millau_runtime::Runtime>::storage_map_final_key(
			account_id,
		))
	}
}

impl TransactionSignScheme for Millau {
	type Chain = Millau;
	type AccountKeyPair = sp_core::sr25519::Pair;
	type SignedTransaction = millau_runtime::UncheckedExtrinsic;

	fn sign_transaction(param: SignParam<Self>) -> Result<Self::SignedTransaction, SubstrateError> {
		let raw_payload = SignedPayload::from_raw(
			param.unsigned.call.clone(),
			(
				frame_system::CheckNonZeroSender::<millau_runtime::Runtime>::new(),
				frame_system::CheckSpecVersion::<millau_runtime::Runtime>::new(),
				frame_system::CheckTxVersion::<millau_runtime::Runtime>::new(),
				frame_system::CheckGenesis::<millau_runtime::Runtime>::new(),
				frame_system::CheckEra::<millau_runtime::Runtime>::from(param.era.frame_era()),
				frame_system::CheckNonce::<millau_runtime::Runtime>::from(param.unsigned.nonce),
				frame_system::CheckWeight::<millau_runtime::Runtime>::new(),
				pallet_transaction_payment::ChargeTransactionPayment::<millau_runtime::Runtime>::from(param.unsigned.tip),
			),
			(
				(),
				param.spec_version,
				param.transaction_version,
				param.genesis_hash,
				param.era.signed_payload(param.genesis_hash),
				(),
				(),
				(),
			),
		);
		let signature = raw_payload.using_encoded(|payload| param.signer.sign(payload));
		let signer: sp_runtime::MultiSigner = param.signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		Ok(millau_runtime::UncheckedExtrinsic::new_signed(
			call.into_decoded()?,
			signer.into_account(),
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
				*address == millau_runtime::Address::from(*signer.public().as_array_ref())
			})
			.unwrap_or(false)
	}

	fn parse_transaction(tx: Self::SignedTransaction) -> Option<UnsignedTransaction<Self::Chain>> {
		let extra = &tx.signature.as_ref()?.2;
		Some(UnsignedTransaction {
			call: tx.function.into(),
			nonce: Compact::<IndexOf<Self::Chain>>::decode(&mut &extra.5.encode()[..]).ok()?.into(),
			tip: Compact::<BalanceOf<Self::Chain>>::decode(&mut &extra.7.encode()[..])
				.ok()?
				.into(),
		})
	}
}

/// Millau signing params.
pub type SigningParams = sp_core::sr25519::Pair;

/// Millau header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<millau_runtime::Header>;

#[cfg(test)]
mod tests {
	use super::*;
	use relay_substrate_client::TransactionEra;

	#[test]
	fn parse_transaction_works() {
		let unsigned = UnsignedTransaction {
			call: millau_runtime::Call::System(millau_runtime::SystemCall::remark {
				remark: b"Hello world!".to_vec(),
			})
			.into(),
			nonce: 777,
			tip: 888,
		};
		let signed_transaction = Millau::sign_transaction(SignParam {
			spec_version: 42,
			transaction_version: 50000,
			genesis_hash: [42u8; 64].into(),
			signer: sp_core::sr25519::Pair::from_seed_slice(&[1u8; 32]).unwrap(),
			era: TransactionEra::immortal(),
			unsigned: unsigned.clone(),
		})
		.unwrap();
		let parsed_transaction = Millau::parse_transaction(signed_transaction).unwrap();
		assert_eq!(parsed_transaction, unsigned);
	}
}
