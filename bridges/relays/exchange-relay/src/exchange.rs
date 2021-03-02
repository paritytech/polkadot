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

//! Relaying proofs of exchange transaction.

use async_trait::async_trait;
use relay_utils::{
	relay_loop::Client as RelayClient, FailedClient, MaybeConnectionError, StringifiedMaybeConnectionError,
};
use std::{
	fmt::{Debug, Display},
	string::ToString,
};

/// Transaction proof pipeline.
pub trait TransactionProofPipeline {
	/// Name of the transaction proof source.
	const SOURCE_NAME: &'static str;
	/// Name of the transaction proof target.
	const TARGET_NAME: &'static str;

	/// Block type.
	type Block: SourceBlock;
	/// Transaction inclusion proof type.
	type TransactionProof;
}

/// Block that is participating in exchange.
pub trait SourceBlock {
	/// Block hash type.
	type Hash: Clone + Debug + Display;
	/// Block number type.
	type Number: Debug
		+ Display
		+ Clone
		+ Copy
		+ Into<u64>
		+ std::cmp::Ord
		+ std::ops::Add<Output = Self::Number>
		+ num_traits::One;
	/// Block transaction.
	type Transaction: SourceTransaction;

	/// Return hash of the block.
	fn id(&self) -> relay_utils::HeaderId<Self::Hash, Self::Number>;
	/// Return block transactions iterator.
	fn transactions(&self) -> Vec<Self::Transaction>;
}

/// Transaction that is participating in exchange.
pub trait SourceTransaction {
	/// Transaction hash type.
	type Hash: Debug + Display;

	/// Return transaction hash.
	fn hash(&self) -> Self::Hash;
}

/// Block hash for given pipeline.
pub type BlockHashOf<P> = <<P as TransactionProofPipeline>::Block as SourceBlock>::Hash;

/// Block number for given pipeline.
pub type BlockNumberOf<P> = <<P as TransactionProofPipeline>::Block as SourceBlock>::Number;

/// Transaction hash for given pipeline.
pub type TransactionOf<P> = <<P as TransactionProofPipeline>::Block as SourceBlock>::Transaction;

/// Transaction hash for given pipeline.
pub type TransactionHashOf<P> = <TransactionOf<P> as SourceTransaction>::Hash;

/// Header id.
pub type HeaderId<P> = relay_utils::HeaderId<BlockHashOf<P>, BlockNumberOf<P>>;

/// Source client API.
#[async_trait]
pub trait SourceClient<P: TransactionProofPipeline>: RelayClient {
	/// Sleep until exchange-related data is (probably) updated.
	async fn tick(&self);
	/// Get block by hash.
	async fn block_by_hash(&self, hash: BlockHashOf<P>) -> Result<P::Block, Self::Error>;
	/// Get canonical block by number.
	async fn block_by_number(&self, number: BlockNumberOf<P>) -> Result<P::Block, Self::Error>;
	/// Return block + index where transaction has been **mined**. May return `Ok(None)` if transaction
	/// is unknown to the source node.
	async fn transaction_block(&self, hash: &TransactionHashOf<P>)
		-> Result<Option<(HeaderId<P>, usize)>, Self::Error>;
	/// Prepare transaction proof.
	async fn transaction_proof(&self, block: &P::Block, tx_index: usize) -> Result<P::TransactionProof, Self::Error>;
}

/// Target client API.
#[async_trait]
pub trait TargetClient<P: TransactionProofPipeline>: RelayClient {
	/// Sleep until exchange-related data is (probably) updated.
	async fn tick(&self);
	/// Returns `Ok(true)` if header is known to the target node.
	async fn is_header_known(&self, id: &HeaderId<P>) -> Result<bool, Self::Error>;
	/// Returns `Ok(true)` if header is finalized by the target node.
	async fn is_header_finalized(&self, id: &HeaderId<P>) -> Result<bool, Self::Error>;
	/// Returns best finalized header id.
	async fn best_finalized_header_id(&self) -> Result<HeaderId<P>, Self::Error>;
	/// Returns `Ok(true)` if transaction proof is need to be relayed.
	async fn filter_transaction_proof(&self, proof: &P::TransactionProof) -> Result<bool, Self::Error>;
	/// Submits transaction proof to the target node.
	async fn submit_transaction_proof(&self, proof: P::TransactionProof) -> Result<(), Self::Error>;
}

/// Block transaction statistics.
#[derive(Debug, Default)]
#[cfg_attr(test, derive(PartialEq))]
pub struct RelayedBlockTransactions {
	/// Total number of transactions processed (either relayed or ignored) so far.
	pub processed: usize,
	/// Total number of transactions successfully relayed so far.
	pub relayed: usize,
	/// Total number of transactions that we have failed to relay so far.
	pub failed: usize,
}

/// Relay all suitable transactions from single block.
///
/// If connection error occurs, returns Err with number of successfully processed transactions.
/// If some other error occurs, it is ignored and other transactions are processed.
///
/// All transaction-level traces are written by this function. This function is not tracing
/// any information about block.
pub async fn relay_block_transactions<P: TransactionProofPipeline>(
	source_client: &impl SourceClient<P>,
	target_client: &impl TargetClient<P>,
	source_block: &P::Block,
	mut relayed_transactions: RelayedBlockTransactions,
) -> Result<RelayedBlockTransactions, (FailedClient, RelayedBlockTransactions)> {
	let transactions_to_process = source_block
		.transactions()
		.into_iter()
		.enumerate()
		.skip(relayed_transactions.processed);
	for (source_tx_index, source_tx) in transactions_to_process {
		let result = async {
			let source_tx_id = format!("{}/{}", source_block.id().1, source_tx_index);
			let source_tx_proof =
				prepare_transaction_proof(source_client, &source_tx_id, source_block, source_tx_index)
					.await
					.map_err(|e| (FailedClient::Source, e))?;

			let needs_to_be_relayed =
				target_client
					.filter_transaction_proof(&source_tx_proof)
					.await
					.map_err(|err| {
						(
							FailedClient::Target,
							StringifiedMaybeConnectionError::new(
								err.is_connection_error(),
								format!("Transaction filtering has failed with {:?}", err),
							),
						)
					})?;

			if !needs_to_be_relayed {
				return Ok(false);
			}

			relay_ready_transaction_proof(target_client, &source_tx_id, source_tx_proof)
				.await
				.map(|_| true)
				.map_err(|e| (FailedClient::Target, e))
		}
		.await;

		// We have two options here:
		// 1) retry with the same transaction later;
		// 2) report error and proceed with next transaction.
		//
		// Option#1 may seems better, but:
		// 1) we do not track if transaction is mined (without an error) by the target node;
		// 2) error could be irrecoverable (e.g. when block is already pruned by bridge module or tx
		//    has invalid format) && we'll end up in infinite loop of retrying the same transaction proof.
		//
		// So we're going with option#2 here (the only exception are connection errors).
		match result {
			Ok(false) => {
				relayed_transactions.processed += 1;
			}
			Ok(true) => {
				log::info!(
					target: "bridge",
					"{} transaction {} proof has been successfully submitted to {} node",
					P::SOURCE_NAME,
					source_tx.hash(),
					P::TARGET_NAME,
				);

				relayed_transactions.processed += 1;
				relayed_transactions.relayed += 1;
			}
			Err((failed_client, err)) => {
				log::error!(
					target: "bridge",
					"Error relaying {} transaction {} proof to {} node: {}. {}",
					P::SOURCE_NAME,
					source_tx.hash(),
					P::TARGET_NAME,
					err.to_string(),
					if err.is_connection_error() {
						"Going to retry after delay..."
					} else {
						"You may need to submit proof of this transaction manually"
					},
				);

				if err.is_connection_error() {
					return Err((failed_client, relayed_transactions));
				}

				relayed_transactions.processed += 1;
				relayed_transactions.failed += 1;
			}
		}
	}

	Ok(relayed_transactions)
}

/// Relay single transaction proof.
pub async fn relay_single_transaction_proof<P: TransactionProofPipeline>(
	source_client: &impl SourceClient<P>,
	target_client: &impl TargetClient<P>,
	source_tx_hash: TransactionHashOf<P>,
) -> Result<(), String> {
	// wait for transaction and header on source node
	let (source_header_id, source_tx_index) = wait_transaction_mined(source_client, &source_tx_hash).await?;
	let source_block = source_client.block_by_hash(source_header_id.1.clone()).await;
	let source_block = source_block.map_err(|err| {
		format!(
			"Error retrieving block {} from {} node: {:?}",
			source_header_id.1,
			P::SOURCE_NAME,
			err,
		)
	})?;

	// wait for transaction and header on target node
	wait_header_imported(target_client, &source_header_id).await?;
	wait_header_finalized(target_client, &source_header_id).await?;

	// and finally - prepare and submit transaction proof to target node
	let source_tx_id = format!("{}", source_tx_hash);
	relay_ready_transaction_proof(
		target_client,
		&source_tx_id,
		prepare_transaction_proof(source_client, &source_tx_id, &source_block, source_tx_index)
			.await
			.map_err(|err| err.to_string())?,
	)
	.await
	.map_err(|err| err.to_string())
}

/// Prepare transaction proof.
async fn prepare_transaction_proof<P: TransactionProofPipeline>(
	source_client: &impl SourceClient<P>,
	source_tx_id: &str,
	source_block: &P::Block,
	source_tx_index: usize,
) -> Result<P::TransactionProof, StringifiedMaybeConnectionError> {
	source_client
		.transaction_proof(source_block, source_tx_index)
		.await
		.map_err(|err| {
			StringifiedMaybeConnectionError::new(
				err.is_connection_error(),
				format!(
					"Error building transaction {} proof on {} node: {:?}",
					source_tx_id,
					P::SOURCE_NAME,
					err,
				),
			)
		})
}

/// Relay prepared proof of transaction.
async fn relay_ready_transaction_proof<P: TransactionProofPipeline>(
	target_client: &impl TargetClient<P>,
	source_tx_id: &str,
	source_tx_proof: P::TransactionProof,
) -> Result<(), StringifiedMaybeConnectionError> {
	target_client
		.submit_transaction_proof(source_tx_proof)
		.await
		.map_err(|err| {
			StringifiedMaybeConnectionError::new(
				err.is_connection_error(),
				format!(
					"Error submitting transaction {} proof to {} node: {:?}",
					source_tx_id,
					P::TARGET_NAME,
					err,
				),
			)
		})
}

/// Wait until transaction is mined by source node.
async fn wait_transaction_mined<P: TransactionProofPipeline>(
	source_client: &impl SourceClient<P>,
	source_tx_hash: &TransactionHashOf<P>,
) -> Result<(HeaderId<P>, usize), String> {
	loop {
		let source_header_and_tx = source_client.transaction_block(&source_tx_hash).await.map_err(|err| {
			format!(
				"Error retrieving transaction {} from {} node: {:?}",
				source_tx_hash,
				P::SOURCE_NAME,
				err,
			)
		})?;
		match source_header_and_tx {
			Some((source_header_id, source_tx)) => {
				log::info!(
					target: "bridge",
					"Transaction {} is retrieved from {} node. Continuing...",
					source_tx_hash,
					P::SOURCE_NAME,
				);

				return Ok((source_header_id, source_tx));
			}
			None => {
				log::info!(
					target: "bridge",
					"Waiting for transaction {} to be mined by {} node...",
					source_tx_hash,
					P::SOURCE_NAME,
				);

				source_client.tick().await;
			}
		}
	}
}

/// Wait until target node imports required header.
async fn wait_header_imported<P: TransactionProofPipeline>(
	target_client: &impl TargetClient<P>,
	source_header_id: &HeaderId<P>,
) -> Result<(), String> {
	loop {
		let is_header_known = target_client.is_header_known(&source_header_id).await.map_err(|err| {
			format!(
				"Failed to check existence of header {}/{} on {} node: {:?}",
				source_header_id.0,
				source_header_id.1,
				P::TARGET_NAME,
				err,
			)
		})?;
		match is_header_known {
			true => {
				log::info!(
					target: "bridge",
					"Header {}/{} is known to {} node. Continuing.",
					source_header_id.0,
					source_header_id.1,
					P::TARGET_NAME,
				);

				return Ok(());
			}
			false => {
				log::info!(
					target: "bridge",
					"Waiting for header {}/{} to be imported by {} node...",
					source_header_id.0,
					source_header_id.1,
					P::TARGET_NAME,
				);

				target_client.tick().await;
			}
		}
	}
}

/// Wait until target node finalizes required header.
async fn wait_header_finalized<P: TransactionProofPipeline>(
	target_client: &impl TargetClient<P>,
	source_header_id: &HeaderId<P>,
) -> Result<(), String> {
	loop {
		let is_header_finalized = target_client
			.is_header_finalized(&source_header_id)
			.await
			.map_err(|err| {
				format!(
					"Failed to check finality of header {}/{} on {} node: {:?}",
					source_header_id.0,
					source_header_id.1,
					P::TARGET_NAME,
					err,
				)
			})?;
		match is_header_finalized {
			true => {
				log::info!(
					target: "bridge",
					"Header {}/{} is finalizd by {} node. Continuing.",
					source_header_id.0,
					source_header_id.1,
					P::TARGET_NAME,
				);

				return Ok(());
			}
			false => {
				log::info!(
					target: "bridge",
					"Waiting for header {}/{} to be finalized by {} node...",
					source_header_id.0,
					source_header_id.1,
					P::TARGET_NAME,
				);

				target_client.tick().await;
			}
		}
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;

	use parking_lot::Mutex;
	use relay_utils::HeaderId;
	use std::{
		collections::{HashMap, HashSet},
		sync::Arc,
	};

	pub fn test_block_id() -> TestHeaderId {
		HeaderId(1, 1)
	}

	pub fn test_next_block_id() -> TestHeaderId {
		HeaderId(2, 2)
	}

	pub fn test_transaction_hash(tx_index: u64) -> TestTransactionHash {
		200 + tx_index
	}

	pub fn test_transaction(tx_index: u64) -> TestTransaction {
		TestTransaction(test_transaction_hash(tx_index))
	}

	pub fn test_block() -> TestBlock {
		TestBlock(test_block_id(), vec![test_transaction(0)])
	}

	pub fn test_next_block() -> TestBlock {
		TestBlock(test_next_block_id(), vec![test_transaction(1)])
	}

	pub type TestBlockNumber = u64;
	pub type TestBlockHash = u64;
	pub type TestTransactionHash = u64;
	pub type TestHeaderId = HeaderId<TestBlockHash, TestBlockNumber>;

	#[derive(Debug, Clone, PartialEq)]
	pub struct TestError(pub bool);

	impl MaybeConnectionError for TestError {
		fn is_connection_error(&self) -> bool {
			self.0
		}
	}

	pub struct TestTransactionProofPipeline;

	impl TransactionProofPipeline for TestTransactionProofPipeline {
		const SOURCE_NAME: &'static str = "TestSource";
		const TARGET_NAME: &'static str = "TestTarget";

		type Block = TestBlock;
		type TransactionProof = TestTransactionProof;
	}

	#[derive(Debug, Clone)]
	pub struct TestBlock(pub TestHeaderId, pub Vec<TestTransaction>);

	impl SourceBlock for TestBlock {
		type Hash = TestBlockHash;
		type Number = TestBlockNumber;
		type Transaction = TestTransaction;

		fn id(&self) -> TestHeaderId {
			self.0
		}

		fn transactions(&self) -> Vec<TestTransaction> {
			self.1.clone()
		}
	}

	#[derive(Debug, Clone)]
	pub struct TestTransaction(pub TestTransactionHash);

	impl SourceTransaction for TestTransaction {
		type Hash = TestTransactionHash;

		fn hash(&self) -> Self::Hash {
			self.0
		}
	}

	#[derive(Debug, Clone, PartialEq)]
	pub struct TestTransactionProof(pub TestTransactionHash);

	#[derive(Clone)]
	pub struct TestTransactionsSource {
		pub on_tick: Arc<dyn Fn(&mut TestTransactionsSourceData) + Send + Sync>,
		pub data: Arc<Mutex<TestTransactionsSourceData>>,
	}

	pub struct TestTransactionsSourceData {
		pub block: Result<TestBlock, TestError>,
		pub transaction_block: Result<Option<(TestHeaderId, usize)>, TestError>,
		pub proofs_to_fail: HashMap<TestTransactionHash, TestError>,
	}

	impl TestTransactionsSource {
		pub fn new(on_tick: Box<dyn Fn(&mut TestTransactionsSourceData) + Send + Sync>) -> Self {
			Self {
				on_tick: Arc::new(on_tick),
				data: Arc::new(Mutex::new(TestTransactionsSourceData {
					block: Ok(test_block()),
					transaction_block: Ok(Some((test_block_id(), 0))),
					proofs_to_fail: HashMap::new(),
				})),
			}
		}
	}

	#[async_trait]
	impl RelayClient for TestTransactionsSource {
		type Error = TestError;

		async fn reconnect(&mut self) -> Result<(), TestError> {
			Ok(())
		}
	}

	#[async_trait]
	impl SourceClient<TestTransactionProofPipeline> for TestTransactionsSource {
		async fn tick(&self) {
			(self.on_tick)(&mut *self.data.lock())
		}

		async fn block_by_hash(&self, _: TestBlockHash) -> Result<TestBlock, TestError> {
			self.data.lock().block.clone()
		}

		async fn block_by_number(&self, _: TestBlockNumber) -> Result<TestBlock, TestError> {
			self.data.lock().block.clone()
		}

		async fn transaction_block(&self, _: &TestTransactionHash) -> Result<Option<(TestHeaderId, usize)>, TestError> {
			self.data.lock().transaction_block.clone()
		}

		async fn transaction_proof(&self, block: &TestBlock, index: usize) -> Result<TestTransactionProof, TestError> {
			let tx_hash = block.1[index].hash();
			let proof_error = self.data.lock().proofs_to_fail.get(&tx_hash).cloned();
			if let Some(err) = proof_error {
				return Err(err);
			}

			Ok(TestTransactionProof(tx_hash))
		}
	}

	#[derive(Clone)]
	pub struct TestTransactionsTarget {
		pub on_tick: Arc<dyn Fn(&mut TestTransactionsTargetData) + Send + Sync>,
		pub data: Arc<Mutex<TestTransactionsTargetData>>,
	}

	pub struct TestTransactionsTargetData {
		pub is_header_known: Result<bool, TestError>,
		pub is_header_finalized: Result<bool, TestError>,
		pub best_finalized_header_id: Result<TestHeaderId, TestError>,
		pub transactions_to_accept: HashSet<TestTransactionHash>,
		pub submitted_proofs: Vec<TestTransactionProof>,
	}

	impl TestTransactionsTarget {
		pub fn new(on_tick: Box<dyn Fn(&mut TestTransactionsTargetData) + Send + Sync>) -> Self {
			Self {
				on_tick: Arc::new(on_tick),
				data: Arc::new(Mutex::new(TestTransactionsTargetData {
					is_header_known: Ok(true),
					is_header_finalized: Ok(true),
					best_finalized_header_id: Ok(test_block_id()),
					transactions_to_accept: vec![test_transaction_hash(0)].into_iter().collect(),
					submitted_proofs: Vec::new(),
				})),
			}
		}
	}

	#[async_trait]
	impl RelayClient for TestTransactionsTarget {
		type Error = TestError;

		async fn reconnect(&mut self) -> Result<(), TestError> {
			Ok(())
		}
	}

	#[async_trait]
	impl TargetClient<TestTransactionProofPipeline> for TestTransactionsTarget {
		async fn tick(&self) {
			(self.on_tick)(&mut *self.data.lock())
		}

		async fn is_header_known(&self, _: &TestHeaderId) -> Result<bool, TestError> {
			self.data.lock().is_header_known.clone()
		}

		async fn is_header_finalized(&self, _: &TestHeaderId) -> Result<bool, TestError> {
			self.data.lock().is_header_finalized.clone()
		}

		async fn best_finalized_header_id(&self) -> Result<TestHeaderId, TestError> {
			self.data.lock().best_finalized_header_id.clone()
		}

		async fn filter_transaction_proof(&self, proof: &TestTransactionProof) -> Result<bool, TestError> {
			Ok(self.data.lock().transactions_to_accept.contains(&proof.0))
		}

		async fn submit_transaction_proof(&self, proof: TestTransactionProof) -> Result<(), TestError> {
			self.data.lock().submitted_proofs.push(proof);
			Ok(())
		}
	}

	fn ensure_relay_single_success(source: &TestTransactionsSource, target: &TestTransactionsTarget) {
		assert_eq!(
			async_std::task::block_on(relay_single_transaction_proof(source, target, test_transaction_hash(0),)),
			Ok(()),
		);
		assert_eq!(
			target.data.lock().submitted_proofs,
			vec![TestTransactionProof(test_transaction_hash(0))],
		);
	}

	fn ensure_relay_single_failure(source: TestTransactionsSource, target: TestTransactionsTarget) {
		assert!(async_std::task::block_on(relay_single_transaction_proof(
			&source,
			&target,
			test_transaction_hash(0),
		))
		.is_err(),);
		assert!(target.data.lock().submitted_proofs.is_empty());
	}

	#[test]
	fn ready_transaction_proof_relayed_immediately() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));
		ensure_relay_single_success(&source, &target)
	}

	#[test]
	fn relay_transaction_proof_waits_for_transaction_to_be_mined() {
		let source = TestTransactionsSource::new(Box::new(|source_data| {
			assert_eq!(source_data.transaction_block, Ok(None));
			source_data.transaction_block = Ok(Some((test_block_id(), 0)));
		}));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		// transaction is not yet mined, but will be available after first wait (tick)
		source.data.lock().transaction_block = Ok(None);

		ensure_relay_single_success(&source, &target)
	}

	#[test]
	fn relay_transaction_fails_when_transaction_retrieval_fails() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		source.data.lock().transaction_block = Err(TestError(false));

		ensure_relay_single_failure(source, target)
	}

	#[test]
	fn relay_transaction_fails_when_proof_retrieval_fails() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		source
			.data
			.lock()
			.proofs_to_fail
			.insert(test_transaction_hash(0), TestError(false));

		ensure_relay_single_failure(source, target)
	}

	#[test]
	fn relay_transaction_proof_waits_for_header_to_be_imported() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|target_data| {
			assert_eq!(target_data.is_header_known, Ok(false));
			target_data.is_header_known = Ok(true);
		}));

		// header is not yet imported, but will be available after first wait (tick)
		target.data.lock().is_header_known = Ok(false);

		ensure_relay_single_success(&source, &target)
	}

	#[test]
	fn relay_transaction_proof_fails_when_is_header_known_fails() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		target.data.lock().is_header_known = Err(TestError(false));

		ensure_relay_single_failure(source, target)
	}

	#[test]
	fn relay_transaction_proof_waits_for_header_to_be_finalized() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|target_data| {
			assert_eq!(target_data.is_header_finalized, Ok(false));
			target_data.is_header_finalized = Ok(true);
		}));

		// header is not yet finalized, but will be available after first wait (tick)
		target.data.lock().is_header_finalized = Ok(false);

		ensure_relay_single_success(&source, &target)
	}

	#[test]
	fn relay_transaction_proof_fails_when_is_header_finalized_fails() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		target.data.lock().is_header_finalized = Err(TestError(false));

		ensure_relay_single_failure(source, target)
	}

	#[test]
	fn relay_transaction_proof_fails_when_target_node_rejects_proof() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		target
			.data
			.lock()
			.transactions_to_accept
			.remove(&test_transaction_hash(0));

		ensure_relay_single_success(&source, &target)
	}

	fn test_relay_block_transactions(
		source: &TestTransactionsSource,
		target: &TestTransactionsTarget,
		pre_relayed: RelayedBlockTransactions,
	) -> Result<RelayedBlockTransactions, RelayedBlockTransactions> {
		async_std::task::block_on(relay_block_transactions(
			source,
			target,
			&TestBlock(
				test_block_id(),
				vec![test_transaction(0), test_transaction(1), test_transaction(2)],
			),
			pre_relayed,
		))
		.map_err(|(_, transactions)| transactions)
	}

	#[test]
	fn relay_block_transactions_process_all_transactions() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		// let's only accept tx#1
		target
			.data
			.lock()
			.transactions_to_accept
			.remove(&test_transaction_hash(0));
		target
			.data
			.lock()
			.transactions_to_accept
			.insert(test_transaction_hash(1));

		let relayed_transactions = test_relay_block_transactions(&source, &target, Default::default());
		assert_eq!(
			relayed_transactions,
			Ok(RelayedBlockTransactions {
				processed: 3,
				relayed: 1,
				failed: 0,
			}),
		);
		assert_eq!(
			target.data.lock().submitted_proofs,
			vec![TestTransactionProof(test_transaction_hash(1))],
		);
	}

	#[test]
	fn relay_block_transactions_ignores_transaction_failure() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		// let's reject proof for tx#0
		source
			.data
			.lock()
			.proofs_to_fail
			.insert(test_transaction_hash(0), TestError(false));

		let relayed_transactions = test_relay_block_transactions(&source, &target, Default::default());
		assert_eq!(
			relayed_transactions,
			Ok(RelayedBlockTransactions {
				processed: 3,
				relayed: 0,
				failed: 1,
			}),
		);
		assert_eq!(target.data.lock().submitted_proofs, vec![],);
	}

	#[test]
	fn relay_block_transactions_fails_on_connection_error() {
		let source = TestTransactionsSource::new(Box::new(|_| unreachable!("no ticks allowed")));
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no ticks allowed")));

		// fail with connection error when preparing proof for tx#1
		source
			.data
			.lock()
			.proofs_to_fail
			.insert(test_transaction_hash(1), TestError(true));

		let relayed_transactions = test_relay_block_transactions(&source, &target, Default::default());
		assert_eq!(
			relayed_transactions,
			Err(RelayedBlockTransactions {
				processed: 1,
				relayed: 1,
				failed: 0,
			}),
		);
		assert_eq!(
			target.data.lock().submitted_proofs,
			vec![TestTransactionProof(test_transaction_hash(0))],
		);

		// now do not fail on tx#2
		source.data.lock().proofs_to_fail.clear();
		// and also relay tx#3
		target
			.data
			.lock()
			.transactions_to_accept
			.insert(test_transaction_hash(2));

		let relayed_transactions = test_relay_block_transactions(&source, &target, relayed_transactions.unwrap_err());
		assert_eq!(
			relayed_transactions,
			Ok(RelayedBlockTransactions {
				processed: 3,
				relayed: 2,
				failed: 0,
			}),
		);
		assert_eq!(
			target.data.lock().submitted_proofs,
			vec![
				TestTransactionProof(test_transaction_hash(0)),
				TestTransactionProof(test_transaction_hash(2))
			],
		);
	}
}
