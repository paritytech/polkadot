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

//! Types used to connect to the Millau-Substrate chain.

use codec::Encode;
use relay_substrate_client::{Chain, ChainBase, ChainWithBalances, Client, TransactionSignScheme};
use sp_core::{storage::StorageKey, Pair};
use sp_runtime::{generic::SignedPayload, traits::IdentifyAccount};
use std::time::Duration;

pub use millau_runtime::BridgeRialtoCall;

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
}

impl Chain for Millau {
	const NAME: &'static str = "Millau";
	const AVERAGE_BLOCK_INTERVAL: Duration = Duration::from_secs(5);

	type AccountId = millau_runtime::AccountId;
	type Index = millau_runtime::Index;
	type SignedBlock = millau_runtime::SignedBlock;
	type Call = millau_runtime::Call;
}

impl ChainWithBalances for Millau {
	type NativeBalance = millau_runtime::Balance;

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
		client: &Client<Self>,
		signer: &Self::AccountKeyPair,
		signer_nonce: <Self::Chain as Chain>::Index,
		call: <Self::Chain as Chain>::Call,
	) -> Self::SignedTransaction {
		let raw_payload = SignedPayload::from_raw(
			call,
			(
				frame_system::CheckSpecVersion::<millau_runtime::Runtime>::new(),
				frame_system::CheckTxVersion::<millau_runtime::Runtime>::new(),
				frame_system::CheckGenesis::<millau_runtime::Runtime>::new(),
				frame_system::CheckEra::<millau_runtime::Runtime>::from(sp_runtime::generic::Era::Immortal),
				frame_system::CheckNonce::<millau_runtime::Runtime>::from(signer_nonce),
				frame_system::CheckWeight::<millau_runtime::Runtime>::new(),
				pallet_transaction_payment::ChargeTransactionPayment::<millau_runtime::Runtime>::from(0),
			),
			(
				millau_runtime::VERSION.spec_version,
				millau_runtime::VERSION.transaction_version,
				*client.genesis_hash(),
				*client.genesis_hash(),
				(),
				(),
				(),
			),
		);
		let signature = raw_payload.using_encoded(|payload| signer.sign(payload));
		let signer: sp_runtime::MultiSigner = signer.public().into();
		let (call, extra, _) = raw_payload.deconstruct();

		millau_runtime::UncheckedExtrinsic::new_signed(call, signer.into_account(), signature.into(), extra)
	}
}

/// Millau signing params.
#[derive(Clone)]
pub struct SigningParams {
	/// Substrate transactions signer.
	pub signer: sp_core::sr25519::Pair,
}

impl SigningParams {
	/// Create signing params from SURI and password.
	pub fn from_suri(suri: &str, password: Option<&str>) -> Result<Self, sp_core::crypto::SecretStringError> {
		Ok(SigningParams {
			signer: sp_core::sr25519::Pair::from_string(suri, password)?,
		})
	}
}

impl std::fmt::Debug for SigningParams {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "{}", self.signer.public())
	}
}

/// Millau header type used in headers sync.
pub type SyncHeader = relay_substrate_client::SyncHeader<millau_runtime::Header>;
