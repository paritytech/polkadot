// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Implements the Chain API Subsystem
//!
//! Provides access to the chain data. Every request may return an error.
//! At the moment, the implementation requires `Client` to implement `HeaderBackend`,
//! we may add more bounds in the future if we will need e.g. block bodies.
//!
//! Supported requests:
//! * Block hash to number
//! * Block hash to header
//! * Finalized block number to hash
//! * Last finalized block number
//! * Ancestors

#![deny(unused_crate_dependencies, unused_results)]
#![warn(missing_docs)]

use polkadot_subsystem::{
	FromOverseer, OverseerSignal,
	SpawnedSubsystem, Subsystem, SubsystemResult, SubsystemError, SubsystemContext,
	messages::ChainApiMessage,
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{Block, BlockId};
use sp_blockchain::HeaderBackend;
use std::sync::Arc;

use futures::prelude::*;

const LOG_TARGET: &str = "parachain::chain-api";

/// The Chain API Subsystem implementation.
pub struct ChainApiSubsystem<Client> {
	client: Arc<Client>,
	metrics: Metrics,
}

impl<Client> ChainApiSubsystem<Client> {
	/// Create a new Chain API subsystem with the given client.
	pub fn new(client: Arc<Client>, metrics: Metrics) -> Self {
		ChainApiSubsystem {
			client,
			metrics,
		}
	}
}

impl<Client, Context> Subsystem<Context> for ChainApiSubsystem<Client> where
	Client: HeaderBackend<Block> + 'static,
	Context: SubsystemContext<Message = ChainApiMessage>
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run(ctx, self)
			.map_err(|e| SubsystemError::with_origin("chain-api", e))
			.boxed();
		SpawnedSubsystem {
			future,
			name: "chain-api-subsystem",
		}
	}
}

async fn run<Client>(
	mut ctx: impl SubsystemContext<Message = ChainApiMessage>,
	subsystem: ChainApiSubsystem<Client>,
) -> SubsystemResult<()>
where
	Client: HeaderBackend<Block>,
{
	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOverseer::Communication { msg } => match msg {
				ChainApiMessage::BlockNumber(hash, response_channel) => {
					let _timer = subsystem.metrics.time_block_number();
					let result = subsystem.client.number(hash).map_err(|e| e.to_string().into());
					subsystem.metrics.on_request(result.is_ok());
					let _ = response_channel.send(result);
				},
				ChainApiMessage::BlockHeader(hash, response_channel) => {
					let _timer = subsystem.metrics.time_block_header();
					let result = subsystem.client
						.header(BlockId::Hash(hash))
						.map_err(|e| e.to_string().into());
					subsystem.metrics.on_request(result.is_ok());
					let _ = response_channel.send(result);
				},
				ChainApiMessage::FinalizedBlockHash(number, response_channel) => {
					let _timer = subsystem.metrics.time_finalized_block_hash();
					// Note: we don't verify it's finalized
					let result = subsystem.client.hash(number).map_err(|e| e.to_string().into());
					subsystem.metrics.on_request(result.is_ok());
					let _ = response_channel.send(result);
				},
				ChainApiMessage::FinalizedBlockNumber(response_channel) => {
					let _timer = subsystem.metrics.time_finalized_block_number();
					let result = subsystem.client.info().finalized_number;
					// always succeeds
					subsystem.metrics.on_request(true);
					let _ = response_channel.send(Ok(result));
				},
				ChainApiMessage::Ancestors { hash, k, response_channel } => {
					let _timer = subsystem.metrics.time_ancestors();
					tracing::span!(tracing::Level::TRACE, "ChainApiMessage::Ancestors", subsystem=LOG_TARGET, hash=%hash, k=k);

					let mut hash = hash;

					let next_parent = core::iter::from_fn(|| {
						let maybe_header = subsystem.client.header(BlockId::Hash(hash));
						match maybe_header {
							// propagate the error
							Err(e) => {
								let e = e.to_string().into();
								Some(Err(e))
							},
							// fewer than `k` ancestors are available
							Ok(None) => None,
							Ok(Some(header)) => {
								// stop at the genesis header.
								if header.number == 1 {
									None
								} else {
									hash = header.parent_hash;
									Some(Ok(hash))
								}
							}
						}
					});

					let result = next_parent.take(k).collect::<Result<Vec<_>, _>>();
					subsystem.metrics.on_request(result.is_ok());
					let _ = response_channel.send(result);
				},
			}
		}
	}
}

#[derive(Clone)]
struct MetricsInner {
	chain_api_requests: prometheus::CounterVec<prometheus::U64>,
	block_number: prometheus::Histogram,
	block_header: prometheus::Histogram,
	finalized_block_hash: prometheus::Histogram,
	finalized_block_number: prometheus::Histogram,
	ancestors: prometheus::Histogram,
}

/// Chain API metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_request(&self, succeeded: bool) {
		if let Some(metrics) = &self.0 {
			if succeeded {
				metrics.chain_api_requests.with_label_values(&["succeeded"]).inc();
			} else {
				metrics.chain_api_requests.with_label_values(&["failed"]).inc();
			}
		}
	}

	/// Provide a timer for `block_number` which observes on drop.
	fn time_block_number(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_number.start_timer())
	}

	/// Provide a timer for `block_header` which observes on drop.
	fn time_block_header(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_header.start_timer())
	}

	/// Provide a timer for `finalized_block_hash` which observes on drop.
	fn time_finalized_block_hash(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.finalized_block_hash.start_timer())
	}

	/// Provide a timer for `finalized_block_number` which observes on drop.
	fn time_finalized_block_number(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.finalized_block_number.start_timer())
	}

	/// Provide a timer for `ancestors` which observes on drop.
	fn time_ancestors(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.ancestors.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			chain_api_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_chain_api_requests_total",
						"Number of Chain API requests served.",
					),
					&["success"],
				)?,
				registry,
			)?,
			block_number: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_chain_api_block_number",
						"Time spent within `chain_api::block_number`",
					)
				)?,
				registry,
			)?,
			block_header: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_chain_api_block_headers",
						"Time spent within `chain_api::block_headers`",
					)
				)?,
				registry,
			)?,
			finalized_block_hash: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_chain_api_finalized_block_hash",
						"Time spent within `chain_api::finalized_block_hash`",
					)
				)?,
				registry,
			)?,
			finalized_block_number: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_chain_api_finalized_block_number",
						"Time spent within `chain_api::finalized_block_number`",
					)
				)?,
				registry,
			)?,
			ancestors: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_chain_api_ancestors",
						"Time spent within `chain_api::ancestors`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}


#[cfg(test)]
mod tests {
	use super::*;

	use std::collections::BTreeMap;
	use futures::{future::BoxFuture, channel::oneshot};

	use polkadot_primitives::v1::{Hash, BlockNumber, BlockId, Header};
	use polkadot_node_subsystem_test_helpers::{make_subsystem_context, TestSubsystemContextHandle};
	use sp_blockchain::Info as BlockInfo;
	use sp_core::testing::TaskExecutor;

	#[derive(Clone)]
	struct TestClient {
		blocks: BTreeMap<Hash, BlockNumber>,
		finalized_blocks: BTreeMap<BlockNumber, Hash>,
		headers: BTreeMap<Hash, Header>,
	}

	const ONE: Hash = Hash::repeat_byte(0x01);
	const TWO: Hash = Hash::repeat_byte(0x02);
	const THREE: Hash = Hash::repeat_byte(0x03);
	const FOUR: Hash = Hash::repeat_byte(0x04);
	const ERROR_PATH: Hash = Hash::repeat_byte(0xFF);

	fn default_header() -> Header {
		Header {
			parent_hash: Hash::zero(),
			number: 100500,
			state_root: Hash::zero(),
			extrinsics_root: Hash::zero(),
			digest: Default::default(),
		}
	}

	impl Default for TestClient {
		fn default() -> Self {
			Self {
				blocks: maplit::btreemap! {
					ONE => 1,
					TWO => 2,
					THREE => 3,
					FOUR => 4,
				},
				finalized_blocks: maplit::btreemap! {
					1 => ONE,
					3 => THREE,
				},
				headers: maplit::btreemap! {
					TWO => Header {
						parent_hash: ONE,
						number: 2,
						..default_header()
					},
					THREE => Header {
						parent_hash: TWO,
						number: 3,
						..default_header()
					},
					FOUR => Header {
						parent_hash: THREE,
						number: 4,
						..default_header()
					},
					ERROR_PATH => Header {
						..default_header()
					}
				}
			}
		}
	}

	fn last_key_value<K: Clone, V: Clone>(map: &BTreeMap<K, V>) -> (K, V) {
		assert!(!map.is_empty());
		map.iter()
			.last()
			.map(|(k, v)| (k.clone(), v.clone()))
			.unwrap()
	}

	impl HeaderBackend<Block> for TestClient {
		fn info(&self) -> BlockInfo<Block> {
			let genesis_hash = self.blocks.iter().next().map(|(h, _)| *h).unwrap();
			let (best_hash, best_number) = last_key_value(&self.blocks);
			let (finalized_number, finalized_hash) = last_key_value(&self.finalized_blocks);

			BlockInfo {
				best_hash,
				best_number,
				genesis_hash,
				finalized_hash,
				finalized_number,
				number_leaves: 0,
				finalized_state: None,
			}
		}
		fn number(&self, hash: Hash) -> sp_blockchain::Result<Option<BlockNumber>> {
			Ok(self.blocks.get(&hash).copied())
		}
		fn hash(&self, number: BlockNumber) -> sp_blockchain::Result<Option<Hash>> {
			Ok(self.finalized_blocks.get(&number).copied())
		}
		fn header(&self, id: BlockId) -> sp_blockchain::Result<Option<Header>> {
			match id {
				// for error path testing
				BlockId::Hash(hash) if hash.is_zero()  => {
					Err(sp_blockchain::Error::Backend("Zero hashes are illegal!".into()))
				}
				BlockId::Hash(hash) => {
					Ok(self.headers.get(&hash).cloned())
				}
				_ => unreachable!(),
			}
		}
		fn status(&self, _id: BlockId) -> sp_blockchain::Result<sp_blockchain::BlockStatus> {
			unimplemented!()
		}
	}

	fn test_harness(
		test: impl FnOnce(Arc<TestClient>, TestSubsystemContextHandle<ChainApiMessage>)
			-> BoxFuture<'static, ()>,
	) {
		let (ctx, ctx_handle) = make_subsystem_context(TaskExecutor::new());
		let client = Arc::new(TestClient::default());

		let subsystem = ChainApiSubsystem::new(client.clone(), Metrics(None));
		let chain_api_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = test(client, ctx_handle);

		futures::executor::block_on(future::join(chain_api_task, test_task));
	}

	#[test]
	fn request_block_number() {
		test_harness(|client, mut sender| {
			async move {
				let zero = Hash::zero();
				let test_cases = [
					(TWO, client.number(TWO).unwrap()),
					(zero, client.number(zero).unwrap()), // not here
				];
				for (hash, expected) in &test_cases {
					let (tx, rx) = oneshot::channel();

					sender.send(FromOverseer::Communication {
						msg: ChainApiMessage::BlockNumber(*hash, tx),
					}).await;

					assert_eq!(rx.await.unwrap().unwrap(), *expected);
				}

				sender.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
			}.boxed()
		})
	}

	#[test]
	fn request_block_header() {
		test_harness(|client, mut sender| {
			async move {
				const NOT_HERE: Hash = Hash::repeat_byte(0x5);
				let test_cases = [
					(TWO, client.header(BlockId::Hash(TWO)).unwrap()),
					(NOT_HERE, client.header(BlockId::Hash(NOT_HERE)).unwrap()),
				];
				for (hash, expected) in &test_cases {
					let (tx, rx) = oneshot::channel();

					sender.send(FromOverseer::Communication {
						msg: ChainApiMessage::BlockHeader(*hash, tx),
					}).await;

					assert_eq!(rx.await.unwrap().unwrap(), *expected);
				}

				sender.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
			}.boxed()
		})
	}

	#[test]
	fn request_finalized_hash() {
		test_harness(|client, mut sender| {
			async move {
				let test_cases = [
					(1, client.hash(1).unwrap()), // not here
					(2, client.hash(2).unwrap()),
				];
				for (number, expected) in &test_cases {
					let (tx, rx) = oneshot::channel();

					sender.send(FromOverseer::Communication {
						msg: ChainApiMessage::FinalizedBlockHash(*number, tx),
					}).await;

					assert_eq!(rx.await.unwrap().unwrap(), *expected);
				}

				sender.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
			}.boxed()
		})
	}

	#[test]
	fn request_last_finalized_number() {
		test_harness(|client, mut sender| {
			async move {
				let (tx, rx) = oneshot::channel();

				let expected = client.info().finalized_number;
				sender.send(FromOverseer::Communication {
					msg: ChainApiMessage::FinalizedBlockNumber(tx),
				}).await;

				assert_eq!(rx.await.unwrap().unwrap(), expected);

				sender.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
			}.boxed()
		})
	}

	#[test]
	fn request_ancestors() {
		test_harness(|_client, mut sender| {
			async move {
				let (tx, rx) = oneshot::channel();
				sender.send(FromOverseer::Communication {
					msg: ChainApiMessage::Ancestors { hash: THREE, k: 4, response_channel: tx },
				}).await;
				assert_eq!(rx.await.unwrap().unwrap(), vec![TWO, ONE]);

				let (tx, rx) = oneshot::channel();
				sender.send(FromOverseer::Communication {
					msg: ChainApiMessage::Ancestors { hash: TWO, k: 1, response_channel: tx },
				}).await;
				assert_eq!(rx.await.unwrap().unwrap(), vec![ONE]);

				let (tx, rx) = oneshot::channel();
				sender.send(FromOverseer::Communication {
					msg: ChainApiMessage::Ancestors { hash: ERROR_PATH, k: 2, response_channel: tx },
				}).await;
				assert!(rx.await.unwrap().is_err());

				sender.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
			}.boxed()
		})
	}
}
