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

//! Relaying proofs of exchange transactions.

use crate::exchange::{
	relay_block_transactions, BlockNumberOf, RelayedBlockTransactions, SourceClient, TargetClient,
	TransactionProofPipeline,
};
use crate::exchange_loop_metrics::ExchangeLoopMetrics;

use backoff::backoff::Backoff;
use futures::{future::FutureExt, select};
use num_traits::One;
use relay_utils::{
	metrics::{start as metrics_start, GlobalMetrics, MetricsParams},
	retry_backoff, FailedClient, MaybeConnectionError,
};
use std::future::Future;

/// Transactions proofs relay state.
#[derive(Debug)]
pub struct TransactionProofsRelayState<BlockNumber> {
	/// Number of last header we have processed so far.
	pub best_processed_header_number: BlockNumber,
}

/// Transactions proofs relay storage.
pub trait TransactionProofsRelayStorage: Clone {
	/// Associated block number.
	type BlockNumber;

	/// Get relay state.
	fn state(&self) -> TransactionProofsRelayState<Self::BlockNumber>;
	/// Update relay state.
	fn set_state(&mut self, state: &TransactionProofsRelayState<Self::BlockNumber>);
}

/// In-memory storage for auto-relay loop.
#[derive(Debug, Clone)]
pub struct InMemoryStorage<BlockNumber> {
	best_processed_header_number: BlockNumber,
}

impl<BlockNumber> InMemoryStorage<BlockNumber> {
	/// Created new in-memory storage with given best processed block number.
	pub fn new(best_processed_header_number: BlockNumber) -> Self {
		InMemoryStorage {
			best_processed_header_number,
		}
	}
}

impl<BlockNumber: Clone + Copy> TransactionProofsRelayStorage for InMemoryStorage<BlockNumber> {
	type BlockNumber = BlockNumber;

	fn state(&self) -> TransactionProofsRelayState<BlockNumber> {
		TransactionProofsRelayState {
			best_processed_header_number: self.best_processed_header_number,
		}
	}

	fn set_state(&mut self, state: &TransactionProofsRelayState<BlockNumber>) {
		self.best_processed_header_number = state.best_processed_header_number;
	}
}

/// Run proofs synchronization.
pub fn run<P: TransactionProofPipeline>(
	storage: impl TransactionProofsRelayStorage<BlockNumber = BlockNumberOf<P>>,
	source_client: impl SourceClient<P>,
	target_client: impl TargetClient<P>,
	metrics_params: Option<MetricsParams>,
	exit_signal: impl Future<Output = ()>,
) {
	let exit_signal = exit_signal.shared();
	let metrics_global = GlobalMetrics::default();
	let metrics_exch = ExchangeLoopMetrics::default();
	let metrics_enabled = metrics_params.is_some();
	metrics_start(
		format!("{}_to_{}_Exchange", P::SOURCE_NAME, P::TARGET_NAME),
		metrics_params,
		&metrics_global,
		&metrics_exch,
	);

	relay_utils::relay_loop::run(
		relay_utils::relay_loop::RECONNECT_DELAY,
		source_client,
		target_client,
		|source_client, target_client| {
			run_until_connection_lost(
				storage.clone(),
				source_client,
				target_client,
				if metrics_enabled {
					Some(metrics_global.clone())
				} else {
					None
				},
				if metrics_enabled {
					Some(metrics_exch.clone())
				} else {
					None
				},
				exit_signal.clone(),
			)
		},
	);
}

/// Run proofs synchronization.
async fn run_until_connection_lost<P: TransactionProofPipeline>(
	mut storage: impl TransactionProofsRelayStorage<BlockNumber = BlockNumberOf<P>>,
	source_client: impl SourceClient<P>,
	target_client: impl TargetClient<P>,
	metrics_global: Option<GlobalMetrics>,
	metrics_exch: Option<ExchangeLoopMetrics>,
	exit_signal: impl Future<Output = ()>,
) -> Result<(), FailedClient> {
	let mut retry_backoff = retry_backoff();
	let mut state = storage.state();
	let mut current_finalized_block = None;

	let exit_signal = exit_signal.fuse();

	futures::pin_mut!(exit_signal);

	loop {
		let iteration_result = run_loop_iteration(
			&mut storage,
			&source_client,
			&target_client,
			&mut state,
			&mut current_finalized_block,
			metrics_exch.as_ref(),
		)
		.await;

		if let Some(ref metrics_global) = metrics_global {
			metrics_global.update().await;
		}

		if let Err((is_connection_error, failed_client)) = iteration_result {
			if is_connection_error {
				return Err(failed_client);
			}

			let retry_timeout = retry_backoff
				.next_backoff()
				.unwrap_or(relay_utils::relay_loop::RECONNECT_DELAY);
			select! {
				_ = async_std::task::sleep(retry_timeout).fuse() => {},
				_ = exit_signal => return Ok(()),
			}
		} else {
			retry_backoff.reset();

			select! {
				_ = source_client.tick().fuse() => {},
				_ = exit_signal => return Ok(()),
			}
		}
	}
}

/// Run exchange loop until we need to break.
async fn run_loop_iteration<P: TransactionProofPipeline>(
	storage: &mut impl TransactionProofsRelayStorage<BlockNumber = BlockNumberOf<P>>,
	source_client: &impl SourceClient<P>,
	target_client: &impl TargetClient<P>,
	state: &mut TransactionProofsRelayState<BlockNumberOf<P>>,
	current_finalized_block: &mut Option<(P::Block, RelayedBlockTransactions)>,
	exchange_loop_metrics: Option<&ExchangeLoopMetrics>,
) -> Result<(), (bool, FailedClient)> {
	let best_finalized_header_id = match target_client.best_finalized_header_id().await {
		Ok(best_finalized_header_id) => {
			log::debug!(
				target: "bridge",
				"Got best finalized {} block from {} node: {:?}",
				P::SOURCE_NAME,
				P::TARGET_NAME,
				best_finalized_header_id,
			);

			best_finalized_header_id
		}
		Err(err) => {
			log::error!(
				target: "bridge",
				"Failed to retrieve best {} header id from {} node: {:?}. Going to retry...",
				P::SOURCE_NAME,
				P::TARGET_NAME,
				err,
			);

			return Err((err.is_connection_error(), FailedClient::Target));
		}
	};

	loop {
		// if we already have some finalized block body, try to relay its transactions
		if let Some((block, relayed_transactions)) = current_finalized_block.take() {
			let result = relay_block_transactions(source_client, target_client, &block, relayed_transactions).await;

			match result {
				Ok(relayed_transactions) => {
					log::info!(
						target: "bridge",
						"Relay has processed {} block #{}. Total/Relayed/Failed transactions: {}/{}/{}",
						P::SOURCE_NAME,
						state.best_processed_header_number,
						relayed_transactions.processed,
						relayed_transactions.relayed,
						relayed_transactions.failed,
					);

					state.best_processed_header_number = state.best_processed_header_number + One::one();
					storage.set_state(state);

					if let Some(ref exchange_loop_metrics) = exchange_loop_metrics {
						exchange_loop_metrics.update::<P>(
							state.best_processed_header_number,
							best_finalized_header_id.0,
							relayed_transactions,
						);
					}

					// we have just updated state => proceed to next block retrieval
				}
				Err((failed_client, relayed_transactions)) => {
					*current_finalized_block = Some((block, relayed_transactions));
					return Err((true, failed_client));
				}
			}
		}

		// we may need to retrieve finalized block body from source node
		if best_finalized_header_id.0 > state.best_processed_header_number {
			let next_block_number = state.best_processed_header_number + One::one();
			let result = source_client.block_by_number(next_block_number).await;

			match result {
				Ok(block) => {
					*current_finalized_block = Some((block, RelayedBlockTransactions::default()));

					// we have received new finalized block => go back to relay its transactions
					continue;
				}
				Err(err) => {
					log::error!(
						target: "bridge",
						"Failed to retrieve canonical block #{} from {} node: {:?}. Going to retry...",
						next_block_number,
						P::SOURCE_NAME,
						err,
					);

					return Err((err.is_connection_error(), FailedClient::Source));
				}
			}
		}

		// there are no any transactions we need to relay => wait for new data
		return Ok(());
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::exchange::tests::{
		test_next_block, test_next_block_id, test_transaction_hash, TestTransactionProof, TestTransactionsSource,
		TestTransactionsTarget,
	};
	use futures::{future::FutureExt, stream::StreamExt};

	#[test]
	fn exchange_loop_is_able_to_relay_proofs() {
		let storage = InMemoryStorage {
			best_processed_header_number: 0,
		};
		let target = TestTransactionsTarget::new(Box::new(|_| unreachable!("no target ticks allowed")));
		let target_data = target.data.clone();
		let (exit_sender, exit_receiver) = futures::channel::mpsc::unbounded();

		let source = TestTransactionsSource::new(Box::new(move |data| {
			let transaction1_relayed = target_data
				.lock()
				.submitted_proofs
				.contains(&TestTransactionProof(test_transaction_hash(0)));
			let transaction2_relayed = target_data
				.lock()
				.submitted_proofs
				.contains(&TestTransactionProof(test_transaction_hash(1)));
			match (transaction1_relayed, transaction2_relayed) {
				(true, true) => exit_sender.unbounded_send(()).unwrap(),
				(true, false) => {
					data.block = Ok(test_next_block());
					target_data.lock().best_finalized_header_id = Ok(test_next_block_id());
					target_data
						.lock()
						.transactions_to_accept
						.insert(test_transaction_hash(1));
				}
				_ => (),
			}
		}));

		run(
			storage,
			source,
			target,
			None,
			exit_receiver.into_future().map(|(_, _)| ()),
		);
	}
}
