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

//! Relaying proofs of PoA -> Substrate exchange transactions.

use crate::instances::BridgeInstance;
use crate::rialto_client::{SubmitEthereumExchangeTransactionProof, SubstrateHighLevelRpc};
use crate::rpc_errors::RpcError;
use crate::substrate_types::into_substrate_ethereum_receipt;

use async_trait::async_trait;
use bp_currency_exchange::MaybeLockFundsTransaction;
use exchange_relay::exchange::{
	relay_single_transaction_proof, SourceBlock, SourceClient, SourceTransaction, TargetClient,
	TransactionProofPipeline,
};
use exchange_relay::exchange_loop::{run as run_loop, InMemoryStorage};
use relay_ethereum_client::{
	types::{
		HeaderId as EthereumHeaderId, HeaderWithTransactions as EthereumHeaderWithTransactions,
		Transaction as EthereumTransaction, TransactionHash as EthereumTransactionHash, H256, HEADER_ID_PROOF,
	},
	Client as EthereumClient, ConnectionParams as EthereumConnectionParams,
};
use relay_rialto_client::{Rialto, SigningParams as RialtoSigningParams};
use relay_substrate_client::{
	Chain as SubstrateChain, Client as SubstrateClient, ConnectionParams as SubstrateConnectionParams,
};
use relay_utils::{metrics::MetricsParams, relay_loop::Client as RelayClient, HeaderId};
use rialto_runtime::exchange::EthereumTransactionInclusionProof;
use std::{sync::Arc, time::Duration};

/// Interval at which we ask Ethereum node for updates.
const ETHEREUM_TICK_INTERVAL: Duration = Duration::from_secs(10);

/// Exchange relay mode.
#[derive(Debug)]
pub enum ExchangeRelayMode {
	/// Relay single transaction and quit.
	Single(EthereumTransactionHash),
	/// Auto-relay transactions starting with given block.
	Auto(Option<u64>),
}

/// PoA exchange transaction relay params.
#[derive(Debug)]
pub struct EthereumExchangeParams {
	/// Ethereum connection params.
	pub eth_params: EthereumConnectionParams,
	/// Substrate connection params.
	pub sub_params: SubstrateConnectionParams,
	/// Substrate signing params.
	pub sub_sign: RialtoSigningParams,
	/// Relay working mode.
	pub mode: ExchangeRelayMode,
	/// Metrics parameters.
	pub metrics_params: Option<MetricsParams>,
	/// Instance of the bridge pallet being synchronized.
	pub instance: Arc<dyn BridgeInstance>,
}

/// Ethereum to Substrate exchange pipeline.
struct EthereumToSubstrateExchange;

impl TransactionProofPipeline for EthereumToSubstrateExchange {
	const SOURCE_NAME: &'static str = "Ethereum";
	const TARGET_NAME: &'static str = "Substrate";

	type Block = EthereumSourceBlock;
	type TransactionProof = EthereumTransactionInclusionProof;
}

/// Ethereum source block.
struct EthereumSourceBlock(EthereumHeaderWithTransactions);

impl SourceBlock for EthereumSourceBlock {
	type Hash = H256;
	type Number = u64;
	type Transaction = EthereumSourceTransaction;

	fn id(&self) -> EthereumHeaderId {
		HeaderId(
			self.0.number.expect(HEADER_ID_PROOF).as_u64(),
			self.0.hash.expect(HEADER_ID_PROOF),
		)
	}

	fn transactions(&self) -> Vec<Self::Transaction> {
		self.0
			.transactions
			.iter()
			.cloned()
			.map(EthereumSourceTransaction)
			.collect()
	}
}

/// Ethereum source transaction.
struct EthereumSourceTransaction(EthereumTransaction);

impl SourceTransaction for EthereumSourceTransaction {
	type Hash = EthereumTransactionHash;

	fn hash(&self) -> Self::Hash {
		self.0.hash
	}
}

/// Ethereum node as transactions proof source.
#[derive(Clone)]
struct EthereumTransactionsSource {
	client: EthereumClient,
}

#[async_trait]
impl RelayClient for EthereumTransactionsSource {
	type Error = RpcError;

	async fn reconnect(&mut self) -> Result<(), RpcError> {
		self.client.reconnect();
		Ok(())
	}
}

#[async_trait]
impl SourceClient<EthereumToSubstrateExchange> for EthereumTransactionsSource {
	async fn tick(&self) {
		async_std::task::sleep(ETHEREUM_TICK_INTERVAL).await;
	}

	async fn block_by_hash(&self, hash: H256) -> Result<EthereumSourceBlock, RpcError> {
		self.client
			.header_by_hash_with_transactions(hash)
			.await
			.map(EthereumSourceBlock)
			.map_err(Into::into)
	}

	async fn block_by_number(&self, number: u64) -> Result<EthereumSourceBlock, RpcError> {
		self.client
			.header_by_number_with_transactions(number)
			.await
			.map(EthereumSourceBlock)
			.map_err(Into::into)
	}

	async fn transaction_block(
		&self,
		hash: &EthereumTransactionHash,
	) -> Result<Option<(EthereumHeaderId, usize)>, RpcError> {
		let eth_tx = match self.client.transaction_by_hash(*hash).await? {
			Some(eth_tx) => eth_tx,
			None => return Ok(None),
		};

		// we need transaction to be mined => check if it is included in the block
		let (eth_header_id, eth_tx_index) = match (eth_tx.block_number, eth_tx.block_hash, eth_tx.transaction_index) {
			(Some(block_number), Some(block_hash), Some(transaction_index)) => (
				HeaderId(block_number.as_u64(), block_hash),
				transaction_index.as_u64() as _,
			),
			_ => return Ok(None),
		};

		Ok(Some((eth_header_id, eth_tx_index)))
	}

	async fn transaction_proof(
		&self,
		block: &EthereumSourceBlock,
		tx_index: usize,
	) -> Result<EthereumTransactionInclusionProof, RpcError> {
		const TRANSACTION_HAS_RAW_FIELD_PROOF: &str = "RPC level checks that transactions from Ethereum\
			node are having `raw` field; qed";
		const BLOCK_HAS_HASH_FIELD_PROOF: &str = "RPC level checks that block has `hash` field; qed";

		let mut transaction_proof = Vec::with_capacity(block.0.transactions.len());
		for tx in &block.0.transactions {
			let raw_tx_receipt = self
				.client
				.transaction_receipt(tx.hash)
				.await
				.map(|receipt| into_substrate_ethereum_receipt(&receipt))
				.map(|receipt| receipt.rlp())?;
			let raw_tx = tx.raw.clone().expect(TRANSACTION_HAS_RAW_FIELD_PROOF).0;
			transaction_proof.push((raw_tx, raw_tx_receipt));
		}

		Ok(EthereumTransactionInclusionProof {
			block: block.0.hash.expect(BLOCK_HAS_HASH_FIELD_PROOF),
			index: tx_index as _,
			proof: transaction_proof,
		})
	}
}

/// Substrate node as transactions proof target.
#[derive(Clone)]
struct SubstrateTransactionsTarget {
	client: SubstrateClient<Rialto>,
	sign_params: RialtoSigningParams,
	bridge_instance: Arc<dyn BridgeInstance>,
}

#[async_trait]
impl RelayClient for SubstrateTransactionsTarget {
	type Error = RpcError;

	async fn reconnect(&mut self) -> Result<(), RpcError> {
		Ok(self.client.reconnect().await?)
	}
}

#[async_trait]
impl TargetClient<EthereumToSubstrateExchange> for SubstrateTransactionsTarget {
	async fn tick(&self) {
		async_std::task::sleep(Rialto::AVERAGE_BLOCK_INTERVAL).await;
	}

	async fn is_header_known(&self, id: &EthereumHeaderId) -> Result<bool, RpcError> {
		self.client.ethereum_header_known(*id).await
	}

	async fn is_header_finalized(&self, id: &EthereumHeaderId) -> Result<bool, RpcError> {
		// we check if header is finalized by simple comparison of the header number and
		// number of best finalized PoA header known to Substrate node.
		//
		// this may lead to failure in tx proof import if PoA reorganization has happened
		// after we have checked that our tx has been included into given block
		//
		// the fix is easy, but since this code is mostly developed for demonstration purposes,
		// I'm leaving this KISS-based design here
		let best_finalized_ethereum_block = self.client.best_ethereum_finalized_block().await?;
		Ok(id.0 <= best_finalized_ethereum_block.0)
	}

	async fn best_finalized_header_id(&self) -> Result<EthereumHeaderId, RpcError> {
		// we can't continue to relay exchange proofs if Substrate node is out of sync, because
		// it may have already received (some of) proofs that we're going to relay
		self.client.ensure_synced().await?;

		self.client.best_ethereum_finalized_block().await
	}

	async fn filter_transaction_proof(&self, proof: &EthereumTransactionInclusionProof) -> Result<bool, RpcError> {
		// let's try to parse transaction locally
		let (raw_tx, raw_tx_receipt) = &proof.proof[proof.index as usize];
		let parse_result = rialto_runtime::exchange::EthTransaction::parse(raw_tx);
		if parse_result.is_err() {
			return Ok(false);
		}

		// now let's check if transaction is successful
		match bp_eth_poa::Receipt::is_successful_raw_receipt(raw_tx_receipt) {
			Ok(true) => (),
			_ => return Ok(false),
		}

		// seems that transaction is relayable - let's check if runtime is able to import it
		// (we can't if e.g. header is pruned or there's some issue with tx data)
		self.client.verify_exchange_transaction_proof(proof.clone()).await
	}

	async fn submit_transaction_proof(&self, proof: EthereumTransactionInclusionProof) -> Result<(), RpcError> {
		let (sign_params, bridge_instance) = (self.sign_params.clone(), self.bridge_instance.clone());
		self.client
			.submit_exchange_transaction_proof(sign_params, bridge_instance, proof)
			.await
	}
}

/// Relay exchange transaction proof(s) to Substrate node.
pub fn run(params: EthereumExchangeParams) {
	match params.mode {
		ExchangeRelayMode::Single(eth_tx_hash) => run_single_transaction_relay(params, eth_tx_hash),
		ExchangeRelayMode::Auto(eth_start_with_block_number) => {
			run_auto_transactions_relay_loop(params, eth_start_with_block_number)
		}
	};
}

/// Run single transaction proof relay and stop.
fn run_single_transaction_relay(params: EthereumExchangeParams, eth_tx_hash: H256) {
	let mut local_pool = futures::executor::LocalPool::new();

	let EthereumExchangeParams {
		eth_params,
		sub_params,
		sub_sign,
		instance,
		..
	} = params;

	let result = local_pool.run_until(async move {
		let eth_client = EthereumClient::new(eth_params);
		let sub_client = SubstrateClient::<Rialto>::new(sub_params)
			.await
			.map_err(RpcError::Substrate)?;

		let source = EthereumTransactionsSource { client: eth_client };
		let target = SubstrateTransactionsTarget {
			client: sub_client,
			sign_params: sub_sign,
			bridge_instance: instance,
		};

		relay_single_transaction_proof(&source, &target, eth_tx_hash).await
	});

	match result {
		Ok(_) => {
			log::info!(
				target: "bridge",
				"Ethereum transaction {} proof has been successfully submitted to Substrate node",
				eth_tx_hash,
			);
		}
		Err(err) => {
			log::error!(
				target: "bridge",
				"Error submitting Ethereum transaction {} proof to Substrate node: {}",
				eth_tx_hash,
				err,
			);
		}
	}
}

/// Run auto-relay loop.
fn run_auto_transactions_relay_loop(params: EthereumExchangeParams, eth_start_with_block_number: Option<u64>) {
	let EthereumExchangeParams {
		eth_params,
		sub_params,
		sub_sign,
		metrics_params,
		instance,
		..
	} = params;

	let do_run_loop = move || -> Result<(), String> {
		let eth_client = EthereumClient::new(eth_params);
		let sub_client = async_std::task::block_on(SubstrateClient::<Rialto>::new(sub_params))
			.map_err(|err| format!("Error starting Substrate client: {:?}", err))?;

		let eth_start_with_block_number = match eth_start_with_block_number {
			Some(eth_start_with_block_number) => eth_start_with_block_number,
			None => {
				async_std::task::block_on(sub_client.best_ethereum_finalized_block())
					.map_err(|err| {
						format!(
							"Error retrieving best finalized Ethereum block from Substrate node: {:?}",
							err
						)
					})?
					.0
			}
		};

		run_loop(
			InMemoryStorage::new(eth_start_with_block_number),
			EthereumTransactionsSource { client: eth_client },
			SubstrateTransactionsTarget {
				client: sub_client,
				sign_params: sub_sign,
				bridge_instance: instance,
			},
			metrics_params,
			futures::future::pending(),
		);

		Ok(())
	};

	if let Err(err) = do_run_loop() {
		log::error!(
			target: "bridge",
			"Error auto-relaying Ethereum transactions proofs to Substrate node: {}",
			err,
		);
	}
}
