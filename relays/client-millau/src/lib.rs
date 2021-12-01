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

use codec::{Compact, Decode, Encode};
use relay_substrate_client::{
	BalanceOf, Chain, ChainBase, ChainWithBalances, IndexOf, TransactionEraOf,
	TransactionSignScheme, UnsignedTransaction,
};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

/// Millau header id.
pub type HeaderId = relay_utils::HeaderId<millau_runtime::Hash, millau_runtime::BlockNumber>;

/// Millau chain definition.
#[derive(Debug, Clone, Copy)]
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
}

impl Chain for Millau {
	const NAME: &'static str = "Millau";
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

	fn sign_transaction(
		genesis_hash: <Self::Chain as ChainBase>::Hash,
		signer: &Self::AccountKeyPair,
		era: TransactionEraOf<Self::Chain>,
		unsigned: UnsignedTransaction<Self::Chain>,
	) -> Self::SignedTransaction {
		let raw_payload = SignedPayload::from_raw(
			unsigned.call,
			(
				frame_system::CheckSpecVersion::<millau_runtime::Runtime>::new(),
				frame_system::CheckTxVersion::<millau_runtime::Runtime>::new(),
				frame_system::CheckGenesis::<millau_runtime::Runtime>::new(),
				frame_system::CheckEra::<millau_runtime::Runtime>::from(era.frame_era()),
				frame_system::CheckNonce::<millau_runtime::Runtime>::from(unsigned.nonce),
				frame_system::CheckWeight::<millau_runtime::Runtime>::new(),
				pallet_transaction_payment::ChargeTransactionPayment::<millau_runtime::Runtime>::from(unsigned.tip),
			),
			(
				millau_runtime::VERSION.spec_version,
				millau_runtime::VERSION.transaction_version,
				genesis_hash,
				era.signed_payload(genesis_hash),
				(),
				(),
				(),
			),
		);
		let signature = raw_payload.using_encoded(|payload| signer.sign(payload));
		let signer: sp_runtime::MultiSigner = signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		millau_runtime::UncheckedExtrinsic::new_signed(
			call,
			signer.into_account(),
			signature.into(),
			extra,
		)
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
			call: tx.function,
			nonce: Compact::<IndexOf<Self::Chain>>::decode(&mut &extra.4.encode()[..]).ok()?.into(),
			tip: Compact::<BalanceOf<Self::Chain>>::decode(&mut &extra.6.encode()[..])
				.ok()?
				.into(),
		})
	}
}

/// Millau signing params.
pub type SigningParams = sp_core::sr25519::Pair;

/// Millau header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<millau_runtime::Header>;
