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

//! Types used to connect to the Westend chain.

use codec::Encode;
use relay_substrate_client::{Chain, ChainBase, ChainWithBalances, TransactionSignScheme};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

/// Westend header id.
pub type HeaderId = relay_utils::HeaderId<bp_westend::Hash, bp_westend::BlockNumber>;

/// Westend header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_westend::Header>;

/// Westend chain definition
#[derive(Debug, Clone, Copy)]
pub struct Westend;

impl ChainBase for Westend {
	type BlockNumber = bp_westend::BlockNumber;
	type Hash = bp_westend::Hash;
	type Hasher = bp_westend::Hasher;
	type Header = bp_westend::Header;
}

impl Chain for Westend {
	const NAME: &'static str = "Westend";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);

	type AccountId = bp_westend::AccountId;
	type Index = bp_westend::Nonce;
	type SignedBlock = bp_westend::SignedBlock;
	type Call = bp_westend::Call;
}

impl ChainWithBalances for Westend {
	type NativeBalance = bp_westend::Balance;

	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		StorageKey(bp_westend::account_info_storage_key(account_id))
	}
}

impl TransactionSignScheme for Westend {
	type Chain = Westend;
	type AccountKeyPair = sp_core::sr25519::Pair;
	type SignedTransaction = bp_westend::UncheckedExtrinsic;

	fn sign_transaction(
		genesis_hash: <Self::Chain as ChainBase>::Hash,
		signer: &Self::AccountKeyPair,
		signer_nonce: <Self::Chain as Chain>::Index,
		call: <Self::Chain as Chain>::Call,
	) -> Self::SignedTransaction {
		let raw_payload = SignedPayload::new(
			call,
			bp_westend::SignedExtensions::new(
				bp_westend::VERSION,
				sp_runtime::generic::Era::Immortal,
				genesis_hash,
				signer_nonce,
				0,
			),
		)
		.expect("SignedExtension never fails.");

		let signature = raw_payload.using_encoded(|payload| signer.sign(payload));
		let signer: sp_runtime::MultiSigner = signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		bp_westend::UncheckedExtrinsic::new_signed(
			call,
			sp_runtime::MultiAddress::Id(signer.into_account()),
			signature.into(),
			extra,
		)
	}
}

/// Westend signing params.
pub type SigningParams = sp_core::sr25519::Pair;
