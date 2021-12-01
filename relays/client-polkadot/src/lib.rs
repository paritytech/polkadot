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

//! Types used to connect to the Polkadot chain.

use codec::Encode;
use relay_substrate_client::{
	Chain, ChainBase, ChainWithBalances, TransactionEraOf, TransactionSignScheme,
	UnsignedTransaction,
};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

pub mod runtime;

/// Polkadot header id.
pub type HeaderId = relay_utils::HeaderId<bp_polkadot::Hash, bp_polkadot::BlockNumber>;

/// Polkadot chain definition
#[derive(Debug, Clone, Copy)]
pub struct Polkadot;

impl ChainBase for Polkadot {
	type BlockNumber = bp_polkadot::BlockNumber;
	type Hash = bp_polkadot::Hash;
	type Hasher = bp_polkadot::Hasher;
	type Header = bp_polkadot::Header;

	type AccountId = bp_polkadot::AccountId;
	type Balance = bp_polkadot::Balance;
	type Index = bp_polkadot::Nonce;
	type Signature = bp_polkadot::Signature;
}

impl Chain for Polkadot {
	const NAME: &'static str = "Polkadot";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);
	const STORAGE_PROOF_OVERHEAD: u32 = bp_polkadot::EXTRA_STORAGE_PROOF_SIZE;
	const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = bp_polkadot::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE;

	type SignedBlock = bp_polkadot::SignedBlock;
	type Call = crate::runtime::Call;
	type WeightToFee = bp_polkadot::WeightToFee;
}

impl ChainWithBalances for Polkadot {
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		StorageKey(bp_polkadot::account_info_storage_key(account_id))
	}
}

impl TransactionSignScheme for Polkadot {
	type Chain = Polkadot;
	type AccountKeyPair = sp_core::sr25519::Pair;
	type SignedTransaction = crate::runtime::UncheckedExtrinsic;

	fn sign_transaction(
		genesis_hash: <Self::Chain as ChainBase>::Hash,
		signer: &Self::AccountKeyPair,
		era: TransactionEraOf<Self::Chain>,
		unsigned: UnsignedTransaction<Self::Chain>,
	) -> Self::SignedTransaction {
		let raw_payload = SignedPayload::new(
			unsigned.call,
			bp_polkadot::SignedExtensions::new(
				bp_polkadot::VERSION,
				era,
				genesis_hash,
				unsigned.nonce,
				unsigned.tip,
			),
		)
		.expect("SignedExtension never fails.");

		let signature = raw_payload.using_encoded(|payload| signer.sign(payload));
		let signer: sp_runtime::MultiSigner = signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		bp_polkadot::UncheckedExtrinsic::new_signed(
			call,
			sp_runtime::MultiAddress::Id(signer.into_account()),
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
				*address == bp_polkadot::AccountId::from(*signer.public().as_array_ref()).into()
			})
			.unwrap_or(false)
	}

	fn parse_transaction(tx: Self::SignedTransaction) -> Option<UnsignedTransaction<Self::Chain>> {
		let extra = &tx.signature.as_ref()?.2;
		Some(UnsignedTransaction { call: tx.function, nonce: extra.nonce(), tip: extra.tip() })
	}
}

/// Polkadot header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_polkadot::Header>;

/// Polkadot signing params.
pub type SigningParams = sp_core::sr25519::Pair;
