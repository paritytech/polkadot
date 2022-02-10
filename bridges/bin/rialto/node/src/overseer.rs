// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! This is almost 1:1 copy of `node/service/src/overseer.rs` file from Polkadot repository.
//! The only exception is that we don't support db upgrades => no `upgrade.rs` module.

// this warning comes from `polkadot_overseer::AllSubsystems` type
#![allow(clippy::type_complexity)]

use crate::service::{AuthorityDiscoveryApi, Error};
use rialto_runtime::{opaque::Block, Hash};

use lru::LruCache;
use polkadot_availability_distribution::IncomingRequestReceivers;
use polkadot_node_core_approval_voting::Config as ApprovalVotingConfig;
use polkadot_node_core_av_store::Config as AvailabilityConfig;
use polkadot_node_core_candidate_validation::Config as CandidateValidationConfig;
use polkadot_node_core_chain_selection::Config as ChainSelectionConfig;
use polkadot_node_core_dispute_coordinator::Config as DisputeCoordinatorConfig;
use polkadot_node_core_provisioner::ProvisionerConfig;
use polkadot_node_network_protocol::request_response::{v1 as request_v1, IncomingRequestReceiver};
use polkadot_overseer::{
	metrics::Metrics as OverseerMetrics, BlockInfo, MetricsTrait, Overseer, InitializedOverseerBuilder,
	OverseerConnector, OverseerHandle,
};
use polkadot_primitives::v1::ParachainHost;
use sc_authority_discovery::Service as AuthorityDiscoveryService;
use sc_client_api::AuxStore;
use sc_keystore::LocalKeystore;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus_babe::BabeApi;
use sp_core::traits::SpawnNamed;
use std::sync::Arc;
use substrate_prometheus_endpoint::Registry;

pub use polkadot_approval_distribution::ApprovalDistribution as ApprovalDistributionSubsystem;
pub use polkadot_availability_bitfield_distribution::BitfieldDistribution as BitfieldDistributionSubsystem;
pub use polkadot_availability_distribution::AvailabilityDistributionSubsystem;
pub use polkadot_availability_recovery::AvailabilityRecoverySubsystem;
pub use polkadot_collator_protocol::{CollatorProtocolSubsystem, ProtocolSide};
pub use polkadot_dispute_distribution::DisputeDistributionSubsystem;
pub use polkadot_gossip_support::GossipSupport as GossipSupportSubsystem;
pub use polkadot_network_bridge::NetworkBridge as NetworkBridgeSubsystem;
pub use polkadot_node_collation_generation::CollationGenerationSubsystem;
pub use polkadot_node_core_approval_voting::ApprovalVotingSubsystem;
pub use polkadot_node_core_av_store::AvailabilityStoreSubsystem;
pub use polkadot_node_core_backing::CandidateBackingSubsystem;
pub use polkadot_node_core_bitfield_signing::BitfieldSigningSubsystem;
pub use polkadot_node_core_candidate_validation::CandidateValidationSubsystem;
pub use polkadot_node_core_chain_api::ChainApiSubsystem;
pub use polkadot_node_core_chain_selection::ChainSelectionSubsystem;
pub use polkadot_node_core_dispute_coordinator::DisputeCoordinatorSubsystem;
pub use polkadot_node_core_provisioner::ProvisionerSubsystem;
pub use polkadot_node_core_runtime_api::RuntimeApiSubsystem;
pub use polkadot_statement_distribution::StatementDistributionSubsystem;

/// Arguments passed for overseer construction.
pub struct OverseerGenArgs<'a, Spawner, RuntimeClient>
where
	RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
	RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
	Spawner: 'static + SpawnNamed + Clone + Unpin,
{
	/// Set of initial relay chain leaves to track.
	pub leaves: Vec<BlockInfo>,
	/// The keystore to use for i.e. validator keys.
	pub keystore: Arc<LocalKeystore>,
	/// Runtime client generic, providing the `ProvieRuntimeApi` trait besides others.
	pub runtime_client: Arc<RuntimeClient>,
	/// The underlying key value store for the parachains.
	pub parachains_db: Arc<dyn kvdb::KeyValueDB>,
	/// Underlying network service implementation.
	pub network_service: Arc<sc_network::NetworkService<Block, Hash>>,
	/// Underlying authority discovery service.
	pub authority_discovery_service: AuthorityDiscoveryService,
	/// POV request receiver
	pub pov_req_receiver: IncomingRequestReceiver<request_v1::PoVFetchingRequest>,
	pub chunk_req_receiver: IncomingRequestReceiver<request_v1::ChunkFetchingRequest>,
	pub collation_req_receiver: IncomingRequestReceiver<request_v1::CollationFetchingRequest>,
	pub available_data_req_receiver:
		IncomingRequestReceiver<request_v1::AvailableDataFetchingRequest>,
	pub statement_req_receiver: IncomingRequestReceiver<request_v1::StatementFetchingRequest>,
	pub dispute_req_receiver: IncomingRequestReceiver<request_v1::DisputeRequest>,
	/// Prometheus registry, commonly used for production systems, less so for test.
	pub registry: Option<&'a Registry>,
	/// Task spawner to be used throughout the overseer and the APIs it provides.
	pub spawner: Spawner,
	/// Configuration for the approval voting subsystem.
	pub approval_voting_config: ApprovalVotingConfig,
	/// Configuration for the availability store subsystem.
	pub availability_config: AvailabilityConfig,
	/// Configuration for the candidate validation subsystem.
	pub candidate_validation_config: CandidateValidationConfig,
	/// Configuration for the chain selection subsystem.
	pub chain_selection_config: ChainSelectionConfig,
	/// Configuration for the dispute coordinator subsystem.
	pub dispute_coordinator_config: DisputeCoordinatorConfig,
	/// Configuration for the provisioner subsystem.
	pub disputes_enabled: bool,
}

/// Obtain a prepared `OverseerBuilder`, that is initialized
/// with all default values.
pub fn prepared_overseer_builder<Spawner, RuntimeClient>(
	OverseerGenArgs {
		leaves,
		keystore,
		runtime_client,
		parachains_db,
		network_service,
		authority_discovery_service,
		pov_req_receiver,
		chunk_req_receiver,
		collation_req_receiver: _,
		available_data_req_receiver,
		statement_req_receiver,
		dispute_req_receiver,
		registry,
		spawner,
		approval_voting_config,
		availability_config,
		candidate_validation_config,
		chain_selection_config,
		dispute_coordinator_config,
		disputes_enabled,
	}: OverseerGenArgs<'_, Spawner, RuntimeClient>,
) -> Result<
	InitializedOverseerBuilder<
		Spawner,
		Arc<RuntimeClient>,
		CandidateValidationSubsystem,
		CandidateBackingSubsystem<Spawner>,
		StatementDistributionSubsystem,
		AvailabilityDistributionSubsystem,
		AvailabilityRecoverySubsystem,
		BitfieldSigningSubsystem<Spawner>,
		BitfieldDistributionSubsystem,
		ProvisionerSubsystem<Spawner>,
		RuntimeApiSubsystem<RuntimeClient>,
		AvailabilityStoreSubsystem,
		NetworkBridgeSubsystem<
			Arc<sc_network::NetworkService<Block, Hash>>,
			AuthorityDiscoveryService,
		>,
		ChainApiSubsystem<RuntimeClient>,
		CollationGenerationSubsystem,
		CollatorProtocolSubsystem,
		ApprovalDistributionSubsystem,
		ApprovalVotingSubsystem,
		GossipSupportSubsystem<AuthorityDiscoveryService>,
		DisputeCoordinatorSubsystem,
		DisputeDistributionSubsystem<AuthorityDiscoveryService>,
		ChainSelectionSubsystem,
	>,
	Error,
>
where
	RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
	RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
	Spawner: 'static + SpawnNamed + Clone + Unpin,
{
	use polkadot_node_subsystem_util::metrics::Metrics;
	use std::iter::FromIterator;

	let metrics = <OverseerMetrics as MetricsTrait>::register(registry)?;

	let builder = Overseer::builder()
		.availability_distribution(AvailabilityDistributionSubsystem::new(
			keystore.clone(),
			IncomingRequestReceivers { pov_req_receiver, chunk_req_receiver },
			Metrics::register(registry)?,
		))
		.availability_recovery(AvailabilityRecoverySubsystem::with_chunks_only(
			available_data_req_receiver,
			Metrics::register(registry)?,
		))
		.availability_store(AvailabilityStoreSubsystem::new(
			parachains_db.clone(),
			availability_config,
			Metrics::register(registry)?,
		))
		.bitfield_distribution(BitfieldDistributionSubsystem::new(Metrics::register(registry)?))
		.bitfield_signing(BitfieldSigningSubsystem::new(
			spawner.clone(),
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.candidate_backing(CandidateBackingSubsystem::new(
			spawner.clone(),
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.candidate_validation(CandidateValidationSubsystem::with_config(
			candidate_validation_config,
			Metrics::register(registry)?, // candidate-validation metrics
			Metrics::register(registry)?, // validation host metrics
		))
		.chain_api(ChainApiSubsystem::new(runtime_client.clone(), Metrics::register(registry)?))
		.collation_generation(CollationGenerationSubsystem::new(Metrics::register(registry)?))
		.collator_protocol(CollatorProtocolSubsystem::new(ProtocolSide::Validator {
			keystore: keystore.clone(),
			eviction_policy: Default::default(),
			metrics: Metrics::register(registry)?,
		}))
		.network_bridge(NetworkBridgeSubsystem::new(
			network_service.clone(),
			authority_discovery_service.clone(),
			Box::new(network_service.clone()),
			Metrics::register(registry)?,
		))
		.provisioner(ProvisionerSubsystem::new(spawner.clone(), ProvisionerConfig { disputes_enabled }, Metrics::register(registry)?))
		.runtime_api(RuntimeApiSubsystem::new(
			runtime_client.clone(),
			Metrics::register(registry)?,
			spawner.clone(),
		))
		.statement_distribution(StatementDistributionSubsystem::new(
			keystore.clone(),
			statement_req_receiver,
			Metrics::register(registry)?,
		))
		.approval_distribution(ApprovalDistributionSubsystem::new(Metrics::register(registry)?))
		.approval_voting(ApprovalVotingSubsystem::with_config(
			approval_voting_config,
			parachains_db.clone(),
			keystore.clone(),
			Box::new(network_service),
			Metrics::register(registry)?,
		))
		.gossip_support(GossipSupportSubsystem::new(
			keystore.clone(),
			authority_discovery_service.clone(),
		))
		.dispute_coordinator(DisputeCoordinatorSubsystem::new(
			parachains_db.clone(),
			dispute_coordinator_config,
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.dispute_distribution(DisputeDistributionSubsystem::new(
			keystore,
			dispute_req_receiver,
			authority_discovery_service,
			Metrics::register(registry)?,
		))
		.chain_selection(ChainSelectionSubsystem::new(chain_selection_config, parachains_db))
		.leaves(Vec::from_iter(
			leaves
				.into_iter()
				.map(|BlockInfo { hash, parent_hash: _, number }| (hash, number)),
		))
		.activation_external_listeners(Default::default())
		.span_per_active_leaf(Default::default())
		.active_leaves(Default::default())
		.supports_parachains(runtime_client)
		.known_leaves(LruCache::new(KNOWN_LEAVES_CACHE_SIZE))
		.metrics(metrics)
		.spawner(spawner);
	Ok(builder)
}

/// Trait for the `fn` generating the overseer.
///
/// Default behavior is to create an unmodified overseer, as `RealOverseerGen`
/// would do.
pub trait OverseerGen {
	/// Overwrite the full generation of the overseer, including the subsystems.
	fn generate<Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'_, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let gen = RealOverseerGen;
		RealOverseerGen::generate::<Spawner, RuntimeClient>(&gen, connector, args)
	}
	// It would be nice to make `create_subsystems` part of this trait,
	// but the amount of generic arguments that would be required as
	// as consequence make this rather annoying to implement and use.
}

use polkadot_overseer::KNOWN_LEAVES_CACHE_SIZE;

/// The regular set of subsystems.
pub struct RealOverseerGen;

impl OverseerGen for RealOverseerGen {
	fn generate<Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'_, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		prepared_overseer_builder(args)?
			.build_with_connector(connector)
			.map_err(|e| e.into())
	}
}
