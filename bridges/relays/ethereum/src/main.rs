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

#![recursion_limit = "1024"]

mod ethereum_client;
mod ethereum_deploy_contract;
mod ethereum_exchange;
mod ethereum_exchange_submit;
mod ethereum_sync_loop;
mod instances;
mod rialto_client;
mod rpc_errors;
mod substrate_sync_loop;
mod substrate_types;

use ethereum_deploy_contract::EthereumDeployContractParams;
use ethereum_exchange::EthereumExchangeParams;
use ethereum_exchange_submit::EthereumExchangeSubmitParams;
use ethereum_sync_loop::EthereumSyncParams;
use headers_relay::sync::TargetTransactionMode;
use hex_literal::hex;
use instances::{BridgeInstance, Kovan, RialtoPoA};
use relay_utils::{initialize::initialize_relay, metrics::MetricsParams};
use secp256k1::SecretKey;
use sp_core::crypto::Pair;
use substrate_sync_loop::SubstrateSyncParams;

use headers_relay::sync::HeadersSyncParams;
use relay_ethereum_client::{ConnectionParams as EthereumConnectionParams, SigningParams as EthereumSigningParams};
use relay_rialto_client::SigningParams as RialtoSigningParams;
use relay_substrate_client::ConnectionParams as SubstrateConnectionParams;
use std::sync::Arc;

fn main() {
	initialize_relay();

	let yaml = clap::load_yaml!("cli.yml");
	let matches = clap::App::from_yaml(yaml).get_matches();
	match matches.subcommand() {
		("eth-to-sub", Some(eth_to_sub_matches)) => {
			log::info!(target: "bridge", "Starting ETH ➡ SUB relay.");
			if ethereum_sync_loop::run(match ethereum_sync_params(&eth_to_sub_matches) {
				Ok(ethereum_sync_params) => ethereum_sync_params,
				Err(err) => {
					log::error!(target: "bridge", "Error parsing parameters: {}", err);
					return;
				}
			})
			.is_err()
			{
				log::error!(target: "bridge", "Unable to get Substrate genesis block for Ethereum sync.");
			};
		}
		("sub-to-eth", Some(sub_to_eth_matches)) => {
			log::info!(target: "bridge", "Starting SUB ➡ ETH relay.");
			if substrate_sync_loop::run(match substrate_sync_params(&sub_to_eth_matches) {
				Ok(substrate_sync_params) => substrate_sync_params,
				Err(err) => {
					log::error!(target: "bridge", "Error parsing parameters: {}", err);
					return;
				}
			})
			.is_err()
			{
				log::error!(target: "bridge", "Unable to get Substrate genesis block for Substrate sync.");
			};
		}
		("eth-deploy-contract", Some(eth_deploy_matches)) => {
			log::info!(target: "bridge", "Deploying ETH contracts.");
			ethereum_deploy_contract::run(match ethereum_deploy_contract_params(&eth_deploy_matches) {
				Ok(ethereum_deploy_params) => ethereum_deploy_params,
				Err(err) => {
					log::error!(target: "bridge", "Error during contract deployment: {}", err);
					return;
				}
			});
		}
		("eth-submit-exchange-tx", Some(eth_exchange_submit_matches)) => {
			log::info!(target: "bridge", "Submitting ETH ➡ SUB exchange transaction.");
			ethereum_exchange_submit::run(match ethereum_exchange_submit_params(&eth_exchange_submit_matches) {
				Ok(eth_exchange_submit_params) => eth_exchange_submit_params,
				Err(err) => {
					log::error!(target: "bridge", "Error submitting Eethereum exchange transaction: {}", err);
					return;
				}
			});
		}
		("eth-exchange-sub", Some(eth_exchange_matches)) => {
			log::info!(target: "bridge", "Starting ETH ➡ SUB exchange transactions relay.");
			ethereum_exchange::run(match ethereum_exchange_params(&eth_exchange_matches) {
				Ok(eth_exchange_params) => eth_exchange_params,
				Err(err) => {
					log::error!(target: "bridge", "Error relaying Ethereum transactions proofs: {}", err);
					return;
				}
			});
		}
		("", _) => {
			log::error!(target: "bridge", "No subcommand specified");
		}
		_ => unreachable!("all possible subcommands are checked above; qed"),
	}
}

fn ethereum_connection_params(matches: &clap::ArgMatches) -> Result<EthereumConnectionParams, String> {
	let mut params = EthereumConnectionParams::default();
	if let Some(eth_host) = matches.value_of("eth-host") {
		params.host = eth_host.into();
	}
	if let Some(eth_port) = matches.value_of("eth-port") {
		params.port = eth_port
			.parse()
			.map_err(|e| format!("Failed to parse eth-port: {}", e))?;
	}
	Ok(params)
}

fn ethereum_signing_params(matches: &clap::ArgMatches) -> Result<EthereumSigningParams, String> {
	let mut params = EthereumSigningParams::default();
	if let Some(eth_signer) = matches.value_of("eth-signer") {
		params.signer =
			SecretKey::parse_slice(&hex::decode(eth_signer).map_err(|e| format!("Failed to parse eth-signer: {}", e))?)
				.map_err(|e| format!("Invalid eth-signer: {}", e))?;
	}
	if let Some(eth_chain_id) = matches.value_of("eth-chain-id") {
		params.chain_id = eth_chain_id
			.parse::<u64>()
			.map_err(|e| format!("Failed to parse eth-chain-id: {}", e))?;
	}
	Ok(params)
}

fn substrate_connection_params(matches: &clap::ArgMatches) -> Result<SubstrateConnectionParams, String> {
	let mut params = SubstrateConnectionParams::default();
	if let Some(sub_host) = matches.value_of("sub-host") {
		params.host = sub_host.into();
	}
	if let Some(sub_port) = matches.value_of("sub-port") {
		params.port = sub_port
			.parse()
			.map_err(|e| format!("Failed to parse sub-port: {}", e))?;
	}
	Ok(params)
}

fn rialto_signing_params(matches: &clap::ArgMatches) -> Result<RialtoSigningParams, String> {
	let mut params = RialtoSigningParams::default();
	if let Some(sub_signer) = matches.value_of("sub-signer") {
		let sub_signer_password = matches.value_of("sub-signer-password");
		params.signer = sp_core::sr25519::Pair::from_string(sub_signer, sub_signer_password)
			.map_err(|e| format!("Failed to parse sub-signer: {:?}", e))?;
	}
	Ok(params)
}

fn ethereum_sync_params(matches: &clap::ArgMatches) -> Result<EthereumSyncParams, String> {
	use crate::ethereum_sync_loop::consts::*;

	let mut sync_params = HeadersSyncParams {
		max_future_headers_to_download: MAX_FUTURE_HEADERS_TO_DOWNLOAD,
		max_headers_in_submitted_status: MAX_SUBMITTED_HEADERS,
		max_headers_in_single_submit: MAX_HEADERS_IN_SINGLE_SUBMIT,
		max_headers_size_in_single_submit: MAX_HEADERS_SIZE_IN_SINGLE_SUBMIT,
		prune_depth: PRUNE_DEPTH,
		target_tx_mode: TargetTransactionMode::Signed,
	};

	match matches.value_of("sub-tx-mode") {
		Some("signed") => sync_params.target_tx_mode = TargetTransactionMode::Signed,
		Some("unsigned") => {
			sync_params.target_tx_mode = TargetTransactionMode::Unsigned;

			// tx pool won't accept too much unsigned transactions
			sync_params.max_headers_in_submitted_status = 10;
		}
		Some("backup") => sync_params.target_tx_mode = TargetTransactionMode::Backup,
		Some(mode) => return Err(format!("Invalid sub-tx-mode: {}", mode)),
		None => sync_params.target_tx_mode = TargetTransactionMode::Signed,
	}

	let params = EthereumSyncParams {
		eth_params: ethereum_connection_params(matches)?,
		sub_params: substrate_connection_params(matches)?,
		sub_sign: rialto_signing_params(matches)?,
		metrics_params: metrics_params(matches)?,
		instance: instance_params(matches)?,
		sync_params,
	};

	log::debug!(target: "bridge", "Ethereum sync params: {:?}", params);

	Ok(params)
}

fn substrate_sync_params(matches: &clap::ArgMatches) -> Result<SubstrateSyncParams, String> {
	use crate::substrate_sync_loop::consts::*;

	let eth_contract_address: relay_ethereum_client::types::Address =
		if let Some(eth_contract) = matches.value_of("eth-contract") {
			eth_contract.parse().map_err(|e| format!("{}", e))?
		} else {
			"731a10897d267e19b34503ad902d0a29173ba4b1"
				.parse()
				.expect("address is hardcoded, thus valid; qed")
		};

	let params = SubstrateSyncParams {
		sub_params: substrate_connection_params(matches)?,
		eth_params: ethereum_connection_params(matches)?,
		eth_sign: ethereum_signing_params(matches)?,
		metrics_params: metrics_params(matches)?,
		sync_params: HeadersSyncParams {
			max_future_headers_to_download: MAX_FUTURE_HEADERS_TO_DOWNLOAD,
			max_headers_in_submitted_status: MAX_SUBMITTED_HEADERS,
			max_headers_in_single_submit: MAX_SUBMITTED_HEADERS,
			max_headers_size_in_single_submit: std::usize::MAX,
			prune_depth: PRUNE_DEPTH,
			target_tx_mode: TargetTransactionMode::Signed,
		},
		eth_contract_address,
	};

	log::debug!(target: "bridge", "Substrate sync params: {:?}", params);

	Ok(params)
}

fn ethereum_deploy_contract_params(matches: &clap::ArgMatches) -> Result<EthereumDeployContractParams, String> {
	let eth_contract_code = parse_hex_argument(matches, "eth-contract-code")?.unwrap_or_else(|| {
		hex::decode(include_str!("../res/substrate-bridge-bytecode.hex")).expect("code is hardcoded, thus valid; qed")
	});
	let sub_initial_authorities_set_id = match matches.value_of("sub-authorities-set-id") {
		Some(sub_initial_authorities_set_id) => Some(
			sub_initial_authorities_set_id
				.parse()
				.map_err(|e| format!("Failed to parse sub-authorities-set-id: {}", e))?,
		),
		None => None,
	};
	let sub_initial_authorities_set = parse_hex_argument(matches, "sub-authorities-set")?;
	let sub_initial_header = parse_hex_argument(matches, "sub-initial-header")?;

	let params = EthereumDeployContractParams {
		eth_params: ethereum_connection_params(matches)?,
		eth_sign: ethereum_signing_params(matches)?,
		sub_params: substrate_connection_params(matches)?,
		sub_initial_authorities_set_id,
		sub_initial_authorities_set,
		sub_initial_header,
		eth_contract_code,
	};

	log::debug!(target: "bridge", "Deploy params: {:?}", params);

	Ok(params)
}

fn ethereum_exchange_submit_params(matches: &clap::ArgMatches) -> Result<EthereumExchangeSubmitParams, String> {
	let eth_nonce = if let Some(eth_nonce) = matches.value_of("eth-nonce") {
		Some(
			relay_ethereum_client::types::U256::from_dec_str(&eth_nonce)
				.map_err(|e| format!("Failed to parse eth-nonce: {}", e))?,
		)
	} else {
		None
	};

	let eth_amount = if let Some(eth_amount) = matches.value_of("eth-amount") {
		eth_amount
			.parse()
			.map_err(|e| format!("Failed to parse eth-amount: {}", e))?
	} else {
		// This is in Wei, represents 1 ETH
		1_000_000_000_000_000_000_u64.into()
	};

	// This is the well-known Substrate account of Ferdie
	let default_recepient = hex!("1cbd2d43530a44705ad088af313e18f80b53ef16b36177cd4b77b846f2a5f07c");

	let sub_recipient = if let Some(sub_recipient) = matches.value_of("sub-recipient") {
		hex::decode(&sub_recipient)
			.map_err(|err| err.to_string())
			.and_then(|vsub_recipient| {
				let expected_len = default_recepient.len();
				if expected_len != vsub_recipient.len() {
					Err(format!("invalid length. Expected {} bytes", expected_len))
				} else {
					let mut sub_recipient = default_recepient;
					sub_recipient.copy_from_slice(&vsub_recipient[..expected_len]);
					Ok(sub_recipient)
				}
			})
			.map_err(|e| format!("Failed to parse sub-recipient: {}", e))?
	} else {
		default_recepient
	};

	let params = EthereumExchangeSubmitParams {
		eth_params: ethereum_connection_params(matches)?,
		eth_sign: ethereum_signing_params(matches)?,
		eth_nonce,
		eth_amount,
		sub_recipient,
	};

	log::debug!(target: "bridge", "Submit Ethereum exchange tx params: {:?}", params);

	Ok(params)
}

fn ethereum_exchange_params(matches: &clap::ArgMatches) -> Result<EthereumExchangeParams, String> {
	let mode = match matches.value_of("eth-tx-hash") {
		Some(eth_tx_hash) => ethereum_exchange::ExchangeRelayMode::Single(
			eth_tx_hash
				.parse()
				.map_err(|e| format!("Failed to parse eth-tx-hash: {}", e))?,
		),
		None => ethereum_exchange::ExchangeRelayMode::Auto(match matches.value_of("eth-start-with-block") {
			Some(eth_start_with_block) => Some(
				eth_start_with_block
					.parse()
					.map_err(|e| format!("Failed to parse eth-start-with-block: {}", e))?,
			),
			None => None,
		}),
	};

	let params = EthereumExchangeParams {
		eth_params: ethereum_connection_params(matches)?,
		sub_params: substrate_connection_params(matches)?,
		sub_sign: rialto_signing_params(matches)?,
		metrics_params: metrics_params(matches)?,
		instance: instance_params(matches)?,
		mode,
	};

	log::debug!(target: "bridge", "Ethereum exchange params: {:?}", params);

	Ok(params)
}

fn metrics_params(matches: &clap::ArgMatches) -> Result<Option<MetricsParams>, String> {
	if matches.is_present("no-prometheus") {
		return Ok(None);
	}

	let mut metrics_params = MetricsParams::default();

	if let Some(prometheus_host) = matches.value_of("prometheus-host") {
		metrics_params.host = prometheus_host.into();
	}
	if let Some(prometheus_port) = matches.value_of("prometheus-port") {
		metrics_params.port = prometheus_port
			.parse()
			.map_err(|e| format!("Failed to parse prometheus-port: {}", e))?;
	}

	Ok(Some(metrics_params))
}

fn instance_params(matches: &clap::ArgMatches) -> Result<Arc<dyn BridgeInstance>, String> {
	let instance = if let Some(instance) = matches.value_of("sub-pallet-instance") {
		match instance.to_lowercase().as_str() {
			"rialto" => Arc::new(RialtoPoA) as Arc<dyn BridgeInstance>,
			"kovan" => Arc::new(Kovan),
			_ => return Err("Unsupported bridge pallet instance".to_string()),
		}
	} else {
		unreachable!("CLI config enforces a default instance, can never be None")
	};

	Ok(instance)
}

fn parse_hex_argument(matches: &clap::ArgMatches, arg: &str) -> Result<Option<Vec<u8>>, String> {
	match matches.value_of(arg) {
		Some(value) => Ok(Some(
			hex::decode(value).map_err(|e| format!("Failed to parse {}: {}", arg, e))?,
		)),
		None => Ok(None),
	}
}

#[cfg(test)]
mod tests {

	// Details: https://github.com/paritytech/parity-bridges-common/issues/118
	#[test]
	fn async_std_sleep_works() {
		let mut local_pool = futures::executor::LocalPool::new();
		local_pool.run_until(async move {
			async_std::task::sleep(std::time::Duration::from_secs(1)).await;
		});
	}
}
