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

//! Support for PoA -> Substrate native tokens exchange.
//!
//! If you want to exchange native PoA tokens for native Substrate
//! chain tokens, you need to:
//! 1) send some PoA tokens to `LOCK_FUNDS_ADDRESS` address on PoA chain. Data field of
//!    the transaction must be SCALE-encoded id of Substrate account that will receive
//!    funds on Substrate chain;
//! 2) wait until the 'lock funds' transaction is mined on PoA chain;
//! 3) wait until the block containing the 'lock funds' transaction is finalized on PoA chain;
//! 4) wait until the required PoA header and its finality are provided
//!    to the PoA -> Substrate bridge module (it can be provided by you);
//! 5) receive tokens by providing proof-of-inclusion of PoA transaction.

use bp_currency_exchange::{
	Error as ExchangeError, LockFundsTransaction, MaybeLockFundsTransaction, Result as ExchangeResult,
};
use bp_eth_poa::{transaction_decode_rlp, RawTransaction, RawTransactionReceipt};
use codec::{Decode, Encode};
use frame_support::RuntimeDebug;
use hex_literal::hex;
use sp_std::vec::Vec;

/// Ethereum address where locked PoA funds must be sent to.
pub const LOCK_FUNDS_ADDRESS: [u8; 20] = hex!("DEADBEEFDEADBEEFDEADBEEFDEADBEEFDEADBEEF");

/// Ethereum transaction inclusion proof.
#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug)]
pub struct EthereumTransactionInclusionProof {
	/// Hash of the block with transaction.
	pub block: sp_core::H256,
	/// Index of the transaction within the block.
	pub index: u64,
	/// The proof itself (right now it is all RLP-encoded transactions of the block +
	/// RLP-encoded receipts of all transactions of the block).
	pub proof: Vec<(RawTransaction, RawTransactionReceipt)>,
}

/// We uniquely identify transfer by the pair (sender, nonce).
///
/// The assumption is that this pair will never appear more than once in
/// transactions included into finalized blocks. This is obviously true
/// for any existing eth-like chain (that keep current tx format), because
/// otherwise transaction can be replayed over and over.
#[derive(Encode, Decode, PartialEq, RuntimeDebug)]
pub struct EthereumTransactionTag {
	/// Account that has locked funds.
	pub account: [u8; 20],
	/// Lock transaction nonce.
	pub nonce: sp_core::U256,
}

/// Eth transaction from runtime perspective.
pub struct EthTransaction;

impl MaybeLockFundsTransaction for EthTransaction {
	type Transaction = RawTransaction;
	type Id = EthereumTransactionTag;
	type Recipient = crate::AccountId;
	type Amount = crate::Balance;

	fn parse(
		raw_tx: &Self::Transaction,
	) -> ExchangeResult<LockFundsTransaction<Self::Id, Self::Recipient, Self::Amount>> {
		let tx = transaction_decode_rlp(raw_tx).map_err(|_| ExchangeError::InvalidTransaction)?;

		// we only accept transactions sending funds directly to the pre-configured address
		if tx.unsigned.to != Some(LOCK_FUNDS_ADDRESS.into()) {
			frame_support::debug::trace!(
				target: "runtime",
				"Failed to parse fund locks transaction. Invalid peer recipient: {:?}",
				tx.unsigned.to,
			);

			return Err(ExchangeError::InvalidTransaction);
		}

		let mut recipient_raw = sp_core::H256::default();
		match tx.unsigned.payload.len() {
			32 => recipient_raw.as_fixed_bytes_mut().copy_from_slice(&tx.unsigned.payload),
			len => {
				frame_support::debug::trace!(
					target: "runtime",
					"Failed to parse fund locks transaction. Invalid recipient length: {}",
					len,
				);

				return Err(ExchangeError::InvalidRecipient);
			}
		}
		let amount = tx.unsigned.value.low_u128();

		if tx.unsigned.value != amount.into() {
			frame_support::debug::trace!(
				target: "runtime",
				"Failed to parse fund locks transaction. Invalid amount: {}",
				tx.unsigned.value,
			);

			return Err(ExchangeError::InvalidAmount);
		}

		Ok(LockFundsTransaction {
			id: EthereumTransactionTag {
				account: *tx.sender.as_fixed_bytes(),
				nonce: tx.unsigned.nonce,
			},
			recipient: crate::AccountId::from(*recipient_raw.as_fixed_bytes()),
			amount,
		})
	}
}

/// Prepares everything required to bench claim of funds locked by given transaction.
#[cfg(feature = "runtime-benchmarks")]
pub(crate) fn prepare_environment_for_claim<T: pallet_bridge_eth_poa::Config<I>, I: frame_support::traits::Instance>(
	transactions: &[(RawTransaction, RawTransactionReceipt)],
) -> bp_eth_poa::H256 {
	use bp_eth_poa::compute_merkle_root;
	use pallet_bridge_eth_poa::{
		test_utils::{insert_dummy_header, validator_utils::validator, HeaderBuilder},
		BridgeStorage, Storage,
	};

	let mut storage = BridgeStorage::<T, I>::new();
	let header = HeaderBuilder::with_parent_number_on_runtime::<T, I>(0)
		.transactions_root(compute_merkle_root(transactions.iter().map(|(tx, _)| tx)))
		.receipts_root(compute_merkle_root(transactions.iter().map(|(_, receipt)| receipt)))
		.sign_by(&validator(0));
	let header_id = header.compute_id();
	insert_dummy_header(&mut storage, header);
	storage.finalize_and_prune_headers(Some(header_id), 0);

	header_id.hash
}

/// Prepare signed ethereum lock-funds transaction.
#[cfg(any(feature = "runtime-benchmarks", test))]
pub(crate) fn prepare_ethereum_transaction(
	recipient: &crate::AccountId,
	editor: impl Fn(&mut bp_eth_poa::UnsignedTransaction),
) -> (RawTransaction, RawTransactionReceipt) {
	use bp_eth_poa::{signatures::SignTransaction, Receipt, TransactionOutcome};

	// prepare tx for OpenEthereum private dev chain:
	// chain id is 0x11
	// sender secret is 0x4d5db4107d237df6a3d58ee5f70ae63d73d7658d4026f2eefd2f204c81682cb7
	let chain_id = 0x11;
	let signer = secp256k1::SecretKey::parse(&hex!(
		"4d5db4107d237df6a3d58ee5f70ae63d73d7658d4026f2eefd2f204c81682cb7"
	))
	.unwrap();
	let recipient_raw: &[u8; 32] = recipient.as_ref();
	let mut eth_tx = bp_eth_poa::UnsignedTransaction {
		nonce: 0.into(),
		to: Some(LOCK_FUNDS_ADDRESS.into()),
		value: 100.into(),
		gas: 100_000.into(),
		gas_price: 100_000.into(),
		payload: recipient_raw.to_vec(),
	};
	editor(&mut eth_tx);
	(
		eth_tx.sign_by(&signer, Some(chain_id)),
		Receipt {
			outcome: TransactionOutcome::StatusCode(1),
			gas_used: Default::default(),
			log_bloom: Default::default(),
			logs: Vec::new(),
		}
		.rlp(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use hex_literal::hex;

	fn ferdie() -> crate::AccountId {
		hex!("1cbd2d43530a44705ad088af313e18f80b53ef16b36177cd4b77b846f2a5f07c").into()
	}

	#[test]
	fn valid_transaction_accepted() {
		assert_eq!(
			EthTransaction::parse(&prepare_ethereum_transaction(&ferdie(), |_| {}).0),
			Ok(LockFundsTransaction {
				id: EthereumTransactionTag {
					account: hex!("00a329c0648769a73afac7f9381e08fb43dbea72"),
					nonce: 0.into(),
				},
				recipient: ferdie(),
				amount: 100,
			}),
		);
	}

	#[test]
	fn invalid_transaction_rejected() {
		assert_eq!(
			EthTransaction::parse(&Vec::new()),
			Err(ExchangeError::InvalidTransaction),
		);
	}

	#[test]
	fn transaction_with_invalid_peer_recipient_rejected() {
		assert_eq!(
			EthTransaction::parse(
				&prepare_ethereum_transaction(&ferdie(), |tx| {
					tx.to = None;
				})
				.0
			),
			Err(ExchangeError::InvalidTransaction),
		);
	}

	#[test]
	fn transaction_with_invalid_recipient_rejected() {
		assert_eq!(
			EthTransaction::parse(
				&prepare_ethereum_transaction(&ferdie(), |tx| {
					tx.payload.clear();
				})
				.0
			),
			Err(ExchangeError::InvalidRecipient),
		);
	}

	#[test]
	fn transaction_with_invalid_amount_rejected() {
		assert_eq!(
			EthTransaction::parse(
				&prepare_ethereum_transaction(&ferdie(), |tx| {
					tx.value = sp_core::U256::from(u128::max_value()) + sp_core::U256::from(1);
				})
				.0
			),
			Err(ExchangeError::InvalidAmount),
		);
	}
}
