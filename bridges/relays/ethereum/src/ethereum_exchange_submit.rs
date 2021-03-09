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

//! Submitting Ethereum -> Substrate exchange transactions.

use bp_eth_poa::{
	signatures::{secret_to_address, SignTransaction},
	UnsignedTransaction,
};
use relay_ethereum_client::{
	types::{CallRequest, U256},
	Client as EthereumClient, ConnectionParams as EthereumConnectionParams, SigningParams as EthereumSigningParams,
};
use rialto_runtime::exchange::LOCK_FUNDS_ADDRESS;

/// Ethereum exchange transaction params.
#[derive(Debug)]
pub struct EthereumExchangeSubmitParams {
	/// Ethereum connection params.
	pub eth_params: EthereumConnectionParams,
	/// Ethereum signing params.
	pub eth_sign: EthereumSigningParams,
	/// Ethereum signer nonce.
	pub eth_nonce: Option<U256>,
	/// Amount of Ethereum tokens to lock.
	pub eth_amount: U256,
	/// Funds recipient on Substrate side.
	pub sub_recipient: [u8; 32],
}

/// Submit single Ethereum -> Substrate exchange transaction.
pub fn run(params: EthereumExchangeSubmitParams) {
	let mut local_pool = futures::executor::LocalPool::new();

	let EthereumExchangeSubmitParams {
		eth_params,
		eth_sign,
		eth_nonce,
		eth_amount,
		sub_recipient,
	} = params;

	let result: Result<_, String> = local_pool.run_until(async move {
		let eth_client = EthereumClient::new(eth_params);

		let eth_signer_address = secret_to_address(&eth_sign.signer);
		let sub_recipient_encoded = sub_recipient;
		let nonce = match eth_nonce {
			Some(eth_nonce) => eth_nonce,
			None => eth_client
				.account_nonce(eth_signer_address)
				.await
				.map_err(|err| format!("error fetching acount nonce: {:?}", err))?,
		};
		let gas = eth_client
			.estimate_gas(CallRequest {
				from: Some(eth_signer_address),
				to: Some(LOCK_FUNDS_ADDRESS.into()),
				value: Some(eth_amount),
				data: Some(sub_recipient_encoded.to_vec().into()),
				..Default::default()
			})
			.await
			.map_err(|err| format!("error estimating gas requirements: {:?}", err))?;
		let eth_tx_unsigned = UnsignedTransaction {
			nonce,
			gas_price: eth_sign.gas_price,
			gas,
			to: Some(LOCK_FUNDS_ADDRESS.into()),
			value: eth_amount,
			payload: sub_recipient_encoded.to_vec(),
		};
		let eth_tx_signed = eth_tx_unsigned
			.clone()
			.sign_by(&eth_sign.signer, Some(eth_sign.chain_id));
		eth_client
			.submit_transaction(eth_tx_signed)
			.await
			.map_err(|err| format!("error submitting transaction: {:?}", err))?;

		Ok(eth_tx_unsigned)
	});

	match result {
		Ok(eth_tx_unsigned) => {
			log::info!(
				target: "bridge",
				"Exchange transaction has been submitted to Ethereum node: {:?}",
				eth_tx_unsigned,
			);
		}
		Err(err) => {
			log::error!(
				target: "bridge",
				"Error submitting exchange transaction to Ethereum node: {}",
				err,
			);
		}
	}
}
