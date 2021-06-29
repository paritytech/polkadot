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

//! Types used to connect to the Rococo-Substrate chain.

use codec::Encode;
use relay_substrate_client::{Chain, ChainBase, ChainWithBalances, TransactionSignScheme};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

pub mod runtime;

/// Rococo header id.
pub type HeaderId = relay_utils::HeaderId<bp_rococo::Hash, bp_rococo::BlockNumber>;

/// Rococo header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<bp_rococo::Header>;

/// Rococo chain definition
#[derive(Debug, Clone, Copy)]
pub struct Rococo;

impl ChainBase for Rococo {
	type BlockNumber = bp_rococo::BlockNumber;
	type Hash = bp_rococo::Hash;
	type Hasher = bp_rococo::Hashing;
	type Header = bp_rococo::Header;
}

impl Chain for Rococo {
	const NAME: &'static str = "Rococo";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(6);

	type AccountId = bp_rococo::AccountId;
	type Index = bp_rococo::Index;
	type SignedBlock = bp_rococo::SignedBlock;
	type Call = crate::runtime::Call;
	type Balance = bp_rococo::Balance;
}

impl ChainWithBalances for Rococo {
	fn account_info_storage_key(account_id: &Self::AccountId) -> StorageKey {
		StorageKey(bp_rococo::account_info_storage_key(account_id))
	}
}

impl TransactionSignScheme for Rococo {
	type Chain = Rococo;
	type AccountKeyPair = sp_core::sr25519::Pair;
	type SignedTransaction = crate::runtime::UncheckedExtrinsic;

	fn sign_transaction(
		genesis_hash: <Self::Chain as ChainBase>::Hash,
		signer: &Self::AccountKeyPair,
		signer_nonce: <Self::Chain as Chain>::Index,
		call: <Self::Chain as Chain>::Call,
	) -> Self::SignedTransaction {
		let raw_payload = SignedPayload::new(
			call,
			bp_rococo::SignedExtensions::new(
				bp_rococo::VERSION,
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

		bp_rococo::UncheckedExtrinsic::new_signed(
			call,
			sp_runtime::MultiAddress::Id(signer.into_account()),
			signature.into(),
			extra,
		)
	}
}

/// Rococo signing params.
pub type SigningParams = sp_core::sr25519::Pair;
