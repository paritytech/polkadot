// Copyright 2017 Parity Technologies (UK) Ltd.
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

#![warn(unused_extern_crates)]

//! Polkadot service. Specialized wrapper over substrate service.

extern crate polkadot_availability_store as av_store;
extern crate polkadot_primitives;
extern crate polkadot_runtime;
extern crate polkadot_executor;
extern crate substrate_primitives as primitives;
#[macro_use]
extern crate substrate_network as network;
extern crate substrate_client as client;
#[macro_use]
extern crate substrate_service as service;
extern crate substrate_consensus_aura as consensus;
extern crate substrate_transaction_pool as transaction_pool;
extern crate tokio;

#[macro_use]
extern crate log;
#[macro_use]
extern crate hex_literal;

pub mod chain_spec;

use std::sync::Arc;
use polkadot_primitives::{parachain, AccountId, Block};
use polkadot_runtime::{GenesisConfig, ClientWithApi};
use tokio::runtime::TaskExecutor;
use service::{FactoryFullConfiguration, FullBackend, LightBackend, FullExecutor, LightExecutor};
use transaction_pool::txpool::{Pool as TransactionPool};
use consensus::{import_queue, start_aura, Config as AuraConfig, AuraImportQueue, NothingExtra};

pub use service::{Roles, PruningMode, TransactionPoolOptions,
	ErrorKind, Error, ComponentBlock, LightComponents, FullComponents,
	FullClient, LightClient, Components, Service, ServiceFactory};
pub use client::ExecutionStrategy;
pub use chain_spec::ChainSpec;

const AURA_SLOT_DURATION: u64 = 6;

construct_simple_protocol! {
	/// Demo protocol attachment for substrate.
	pub struct PolkadotProtocol where Block = Block { }
}

/// All configuration for the polkadot node.
pub type Configuration = FactoryFullConfiguration<Factory>;

/// Polkadot-specific configuration.
#[derive(Default)]
pub struct CustomConfiguration {
	/// Set to `Some` with a collator `AccountId` and desired parachain
	/// if the network protocol should be started in collator mode.
	pub collating_for: Option<(AccountId, parachain::Id)>,
}

construct_service_factory! {
	struct Factory {
		Block = Block,
		RuntimeApi = ClientWithApi,
		NetworkProtocol = PolkadotProtocol { |config| Ok(PolkadotProtocol::new()) },
		RuntimeDispatch = polkadot_executor::Executor,
		FullTransactionPoolApi = transaction_pool::ChainApi<client::Client<FullBackend<Self>, FullExecutor<Self>, Block, ClientWithApi>, Block>
			{ |config, client| Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client))) },
		LightTransactionPoolApi = transaction_pool::ChainApi<client::Client<LightBackend<Self>, LightExecutor<Self>, Block, ClientWithApi>, Block>
			{ |config, client| Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client))) },
		Genesis = GenesisConfig,
		Configuration = CustomConfiguration,
		FullService = FullComponents<Self>
			{ |config: FactoryFullConfiguration<Self>, executor: TaskExecutor| {
				let is_auth = config.roles == Roles::AUTHORITY;
				FullComponents::<Factory>::new(config, executor.clone()).map(move |service|{
					if is_auth {
						if let Ok(Some(Ok(key))) = service.keystore().contents()
							.map(|keys| keys.get(0).map(|k| service.keystore().load(k, "")))
						{
							info!("Using authority key {}", key.public());
							let task = start_aura(
								AuraConfig {
									local_key:  Some(Arc::new(key)),
									slot_duration: AURA_SLOT_DURATION,
								},
								service.client(),
								service.proposer(),
								service.network(),
							);

							executor.spawn(task);
						}
					}

					service
				})
			}
		},
		LightService = LightComponents<Self>
			{ |config, executor| <LightComponents<Factory>>::new(config, executor) },
		FullImportQueue = AuraImportQueue<Self::Block, FullClient<Self>, NothingExtra>
			{ |config, client| Ok(import_queue(
				AuraConfig {
					local_key: None,
					slot_duration: 5
				},
				client,
				NothingExtra,
			))
			},
		LightImportQueue = AuraImportQueue<Self::Block, LightClient<Self>, NothingExtra>
			{ |config, client| Ok(import_queue(
				AuraConfig {
					local_key: None,
					slot_duration: 5
				},
				client,
				NothingExtra,
			))
			},
	}
}

