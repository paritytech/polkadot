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

use crate::types::{Address, CallRequest, U256};
use crate::{Client, Result};
use bp_eth_poa::signatures::{secret_to_address, SignTransaction};
use hex_literal::hex;
use secp256k1::SecretKey;

/// Ethereum signing params.
#[derive(Clone, Debug)]
pub struct SigningParams {
	/// Ethereum chain id.
	pub chain_id: u64,
	/// Ethereum transactions signer.
	pub signer: SecretKey,
	/// Gas price we agree to pay.
	pub gas_price: U256,
}

impl Default for SigningParams {
	fn default() -> Self {
		SigningParams {
			chain_id: 0x11, // Parity dev chain
			// account that has a lot of ether when we run instant seal engine
			// address: 0x00a329c0648769a73afac7f9381e08fb43dbea72
			// secret: 0x4d5db4107d237df6a3d58ee5f70ae63d73d7658d4026f2eefd2f204c81682cb7
			signer: SecretKey::parse(&hex!(
				"4d5db4107d237df6a3d58ee5f70ae63d73d7658d4026f2eefd2f204c81682cb7"
			))
			.expect("secret is hardcoded, thus valid; qed"),
			gas_price: 8_000_000_000u64.into(), // 8 Gwei
		}
	}
}

/// Sign and submit tranaction using given Ethereum client.
pub async fn sign_and_submit_transaction(
	client: &Client,
	params: &SigningParams,
	contract_address: Option<Address>,
	nonce: Option<U256>,
	double_gas: bool,
	encoded_call: Vec<u8>,
) -> Result<()> {
	let nonce = if let Some(n) = nonce {
		n
	} else {
		let address: Address = secret_to_address(&params.signer);
		client.account_nonce(address).await?
	};

	let call_request = CallRequest {
		to: contract_address,
		data: Some(encoded_call.clone().into()),
		..Default::default()
	};
	let gas = client.estimate_gas(call_request).await?;

	let raw_transaction = bp_eth_poa::UnsignedTransaction {
		nonce,
		to: contract_address,
		value: U256::zero(),
		gas: if double_gas { gas.saturating_mul(2.into()) } else { gas },
		gas_price: params.gas_price,
		payload: encoded_call,
	}
	.sign_by(&params.signer, Some(params.chain_id));

	let _ = client.submit_transaction(raw_transaction).await?;
	Ok(())
}
