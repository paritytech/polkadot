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

use crate::ethereum_client::{bridge_contract, EthereumHighLevelRpc};
use crate::rpc_errors::RpcError;

use codec::{Decode, Encode};
use num_traits::Zero;
use relay_ethereum_client::{
	Client as EthereumClient, ConnectionParams as EthereumConnectionParams, SigningParams as EthereumSigningParams,
};
use relay_rialto_client::{HeaderId as RialtoHeaderId, Rialto};
use relay_substrate_client::{
	Client as SubstrateClient, ConnectionParams as SubstrateConnectionParams, OpaqueGrandpaAuthoritiesSet,
};
use relay_utils::HeaderId;

/// Ethereum synchronization parameters.
#[derive(Debug)]
pub struct EthereumDeployContractParams {
	/// Ethereum connection params.
	pub eth_params: EthereumConnectionParams,
	/// Ethereum signing params.
	pub eth_sign: EthereumSigningParams,
	/// Ethereum contract bytecode.
	pub eth_contract_code: Vec<u8>,
	/// Substrate connection params.
	pub sub_params: SubstrateConnectionParams,
	/// Initial authorities set id.
	pub sub_initial_authorities_set_id: Option<u64>,
	/// Initial authorities set.
	pub sub_initial_authorities_set: Option<Vec<u8>>,
	/// Initial header.
	pub sub_initial_header: Option<Vec<u8>>,
}

/// Deploy Bridge contract on Ethereum chain.
pub fn run(params: EthereumDeployContractParams) {
	let mut local_pool = futures::executor::LocalPool::new();

	let EthereumDeployContractParams {
		eth_params,
		eth_sign,
		sub_params,
		sub_initial_authorities_set_id,
		sub_initial_authorities_set,
		sub_initial_header,
		eth_contract_code,
	} = params;

	let result = local_pool.run_until(async move {
		let eth_client = EthereumClient::new(eth_params);
		let sub_client = SubstrateClient::<Rialto>::new(sub_params).await.map_err(RpcError::Substrate)?;

		let (initial_header_id, initial_header) = prepare_initial_header(&sub_client, sub_initial_header).await?;
		let initial_set_id = sub_initial_authorities_set_id.unwrap_or(0);
		let initial_set = prepare_initial_authorities_set(
			&sub_client,
			initial_header_id.1,
			sub_initial_authorities_set,
		).await?;

		log::info!(
			target: "bridge",
			"Deploying Ethereum contract.\r\n\tInitial header: {:?}\r\n\tInitial header id: {:?}\r\n\tInitial header encoded: {}\r\n\tInitial authorities set ID: {}\r\n\tInitial authorities set: {}",
			initial_header,
			initial_header_id,
			hex::encode(&initial_header),
			initial_set_id,
			hex::encode(&initial_set),
		);

		deploy_bridge_contract(
			&eth_client,
			&eth_sign,
			eth_contract_code,
			initial_header,
			initial_set_id,
			initial_set,
		).await
	});

	if let Err(error) = result {
		log::error!(target: "bridge", "{}", error);
	}
}

/// Prepare initial header.
async fn prepare_initial_header(
	sub_client: &SubstrateClient<Rialto>,
	sub_initial_header: Option<Vec<u8>>,
) -> Result<(RialtoHeaderId, Vec<u8>), String> {
	match sub_initial_header {
		Some(raw_initial_header) => match rialto_runtime::Header::decode(&mut &raw_initial_header[..]) {
			Ok(initial_header) => Ok((
				HeaderId(initial_header.number, initial_header.hash()),
				raw_initial_header,
			)),
			Err(error) => Err(format!("Error decoding initial header: {}", error)),
		},
		None => {
			let initial_header = sub_client.header_by_number(Zero::zero()).await;
			initial_header
				.map(|header| (HeaderId(Zero::zero(), header.hash()), header.encode()))
				.map_err(|error| format!("Error reading Substrate genesis header: {:?}", error))
		}
	}
}

/// Prepare initial GRANDPA authorities set.
async fn prepare_initial_authorities_set(
	sub_client: &SubstrateClient<Rialto>,
	sub_initial_header_hash: rialto_runtime::Hash,
	sub_initial_authorities_set: Option<Vec<u8>>,
) -> Result<OpaqueGrandpaAuthoritiesSet, String> {
	let initial_authorities_set = match sub_initial_authorities_set {
		Some(initial_authorities_set) => Ok(initial_authorities_set),
		None => sub_client.grandpa_authorities_set(sub_initial_header_hash).await,
	};

	initial_authorities_set.map_err(|error| format!("Error reading GRANDPA authorities set: {:?}", error))
}

/// Deploy bridge contract to Ethereum chain.
async fn deploy_bridge_contract(
	eth_client: &EthereumClient,
	params: &EthereumSigningParams,
	contract_code: Vec<u8>,
	initial_header: Vec<u8>,
	initial_set_id: u64,
	initial_authorities: Vec<u8>,
) -> Result<(), String> {
	eth_client
		.submit_ethereum_transaction(
			params,
			None,
			None,
			false,
			bridge_contract::constructor(contract_code, initial_header, initial_set_id, initial_authorities),
		)
		.await
		.map_err(|error| format!("Error deploying contract: {:?}", error))
}
