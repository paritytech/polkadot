// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

use super::{AuthorityDiscoveryApi, Block, Error, Hash, IsCollator, Registry};
use sp_core::traits::SpawnNamed;

use lru::LruCache;
use polkadot_availability_distribution::IncomingRequestReceivers;
use polkadot_node_core_approval_voting::Config as ApprovalVotingConfig;
use polkadot_node_core_av_store::Config as AvailabilityConfig;
use polkadot_node_core_candidate_validation::Config as CandidateValidationConfig;
use polkadot_node_core_chain_selection::Config as ChainSelectionConfig;
use polkadot_node_core_dispute_coordinator::Config as DisputeCoordinatorConfig;
use polkadot_node_network_protocol::{
	peer_set::PeerSetProtocolNames,
	request_response::{v1 as request_v1, IncomingRequestReceiver, ReqProtocolNames},
};
#[cfg(any(feature = "malus", test))]
pub use polkadot_overseer::{
	dummy::{dummy_overseer_builder, DummySubsystem},
	HeadSupportsParachains,
};
use polkadot_overseer::{
	metrics::Metrics as OverseerMetrics, BlockInfo, InitializedOverseerBuilder, MetricsTrait,
	Overseer, OverseerConnector, OverseerHandle, SpawnGlue,
};

use polkadot_primitives::runtime_api::ParachainHost;
use sc_authority_discovery::Service as AuthorityDiscoveryService;
use sc_client_api::AuxStore;
use sc_keystore::LocalKeystore;
use sc_network_common::service::NetworkStateInfo;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus_babe::BabeApi;
use std::sync::Arc;

pub use polkadot_approval_distribution::ApprovalDistribution as ApprovalDistributionSubsystem;
pub use polkadot_availability_bitfield_distribution::BitfieldDistribution as BitfieldDistributionSubsystem;
pub use polkadot_availability_distribution::AvailabilityDistributionSubsystem;
pub use polkadot_availability_recovery::AvailabilityRecoverySubsystem;
pub use polkadot_collator_protocol::{CollatorProtocolSubsystem, ProtocolSide};
pub use polkadot_dispute_distribution::DisputeDistributionSubsystem;
pub use polkadot_gossip_support::GossipSupport as GossipSupportSubsystem;
pub use polkadot_network_bridge::{
	Metrics as NetworkBridgeMetrics, NetworkBridgeRx as NetworkBridgeRxSubsystem,
	NetworkBridgeTx as NetworkBridgeTxSubsystem,
};
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
pub use polkadot_node_core_pvf_checker::PvfCheckerSubsystem;
pub use polkadot_node_core_runtime_api::RuntimeApiSubsystem;
use polkadot_node_subsystem_util::rand::{self, SeedableRng};
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
	pub parachains_db: Arc<dyn polkadot_node_subsystem_util::database::Database>,
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
	/// Determines the behavior of the collator.
	pub is_collator: IsCollator,
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
	/// Enable PVF pre-checking
	pub pvf_checker_enabled: bool,
	/// Overseer channel capacity override.
	pub overseer_message_channel_capacity_override: Option<usize>,
	/// Request-response protocol names source.
	pub req_protocol_names: ReqProtocolNames,
	/// [`PeerSet`] protocol names to protocols mapping.
	pub peerset_protocol_names: PeerSetProtocolNames,
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
		collation_req_receiver,
		available_data_req_receiver,
		statement_req_receiver,
		dispute_req_receiver,
		registry,
		spawner,
		is_collator,
		approval_voting_config,
		availability_config,
		candidate_validation_config,
		chain_selection_config,
		dispute_coordinator_config,
		pvf_checker_enabled,
		overseer_message_channel_capacity_override,
		req_protocol_names,
		peerset_protocol_names,
	}: OverseerGenArgs<Spawner, RuntimeClient>,
) -> Result<
	InitializedOverseerBuilder<
		SpawnGlue<Spawner>,
		Arc<RuntimeClient>,
		CandidateValidationSubsystem,
		PvfCheckerSubsystem,
		CandidateBackingSubsystem,
		StatementDistributionSubsystem<rand::rngs::StdRng>,
		AvailabilityDistributionSubsystem,
		AvailabilityRecoverySubsystem,
		BitfieldSigningSubsystem,
		BitfieldDistributionSubsystem,
		ProvisionerSubsystem,
		RuntimeApiSubsystem<RuntimeClient>,
		AvailabilityStoreSubsystem,
		NetworkBridgeRxSubsystem<
			Arc<sc_network::NetworkService<Block, Hash>>,
			AuthorityDiscoveryService,
		>,
		NetworkBridgeTxSubsystem<
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

	let metrics = <OverseerMetrics as MetricsTrait>::register(registry)?;

	let spawner = SpawnGlue(spawner);

	let network_bridge_metrics: NetworkBridgeMetrics = Metrics::register(registry)?;

	let builder = Overseer::builder()
		.network_bridge_tx(NetworkBridgeTxSubsystem::new(
			network_service.clone(),
			authority_discovery_service.clone(),
			network_bridge_metrics.clone(),
			req_protocol_names,
			peerset_protocol_names.clone(),
		))
		.network_bridge_rx(NetworkBridgeRxSubsystem::new(
			network_service.clone(),
			authority_discovery_service.clone(),
			Box::new(network_service.clone()),
			network_bridge_metrics,
			peerset_protocol_names,
		))
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
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.candidate_backing(CandidateBackingSubsystem::new(
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.candidate_validation(CandidateValidationSubsystem::with_config(
			candidate_validation_config,
			Metrics::register(registry)?, // candidate-validation metrics
			Metrics::register(registry)?, // validation host metrics
		))
		.pvf_checker(PvfCheckerSubsystem::new(
			pvf_checker_enabled,
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.chain_api(ChainApiSubsystem::new(runtime_client.clone(), Metrics::register(registry)?))
		.collation_generation(CollationGenerationSubsystem::new(Metrics::register(registry)?))
		.collator_protocol({
			let side = match is_collator {
				IsCollator::Yes(collator_pair) => ProtocolSide::Collator(
					network_service.local_peer_id(),
					collator_pair,
					collation_req_receiver,
					Metrics::register(registry)?,
				),
				IsCollator::No => ProtocolSide::Validator {
					keystore: keystore.clone(),
					eviction_policy: Default::default(),
					metrics: Metrics::register(registry)?,
				},
			};
			CollatorProtocolSubsystem::new(side)
		})
		.provisioner(ProvisionerSubsystem::new(Metrics::register(registry)?))
		.runtime_api(RuntimeApiSubsystem::new(
			runtime_client.clone(),
			Metrics::register(registry)?,
			spawner.clone(),
		))
		.statement_distribution(StatementDistributionSubsystem::new(
			keystore.clone(),
			statement_req_receiver,
			Metrics::register(registry)?,
			rand::rngs::StdRng::from_entropy(),
		))
		.approval_distribution(ApprovalDistributionSubsystem::new(Metrics::register(registry)?))
		.approval_voting(ApprovalVotingSubsystem::with_config(
			approval_voting_config,
			parachains_db.clone(),
			keystore.clone(),
			Box::new(network_service.clone()),
			Metrics::register(registry)?,
		))
		.gossip_support(GossipSupportSubsystem::new(
			keystore.clone(),
			authority_discovery_service.clone(),
			Metrics::register(registry)?,
		))
		.dispute_coordinator(DisputeCoordinatorSubsystem::new(
			parachains_db.clone(),
			dispute_coordinator_config,
			keystore.clone(),
			Metrics::register(registry)?,
		))
		.dispute_distribution(DisputeDistributionSubsystem::new(
			keystore.clone(),
			dispute_req_receiver,
			authority_discovery_service.clone(),
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

	if let Some(capacity) = overseer_message_channel_capacity_override {
		Ok(builder.message_channel_capacity(capacity))
	} else {
		Ok(builder)
	}
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
		args: OverseerGenArgs<Spawner, RuntimeClient>,
	) -> Result<(Overseer<SpawnGlue<Spawner>, Arc<RuntimeClient>>, OverseerHandle), Error>
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
		args: OverseerGenArgs<Spawner, RuntimeClient>,
	) -> Result<(Overseer<SpawnGlue<Spawner>, Arc<RuntimeClient>>, OverseerHandle), Error>
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
