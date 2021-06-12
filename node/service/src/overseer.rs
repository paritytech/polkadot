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

use super::{
	Error,
	Registry,
	IsCollator,
	Block,
	SpawnNamed,
	Hash,
	AuthorityDiscoveryApi,
};
use std::sync::Arc;
use	polkadot_network_bridge::RequestMultiplexer;
use	polkadot_node_core_av_store::Config as AvailabilityConfig;
use	polkadot_node_core_approval_voting::Config as ApprovalVotingConfig;
use	polkadot_node_core_candidate_validation::Config as CandidateValidationConfig;
use	polkadot_overseer::{AllSubsystems, BlockInfo, Overseer, OverseerHandler};
use	polkadot_primitives::v1::ParachainHost;
use	sc_authority_discovery::Service as AuthorityDiscoveryService;
use sp_api::ProvideRuntimeApi;
use	sp_blockchain::HeaderBackend;
use	sc_client_api::AuxStore;
use	sc_keystore::LocalKeystore;
use	sp_consensus_babe::BabeApi;

/// Arguments passed for overseer construction.
pub struct OverseerGenArgs<'a, Spawner, RuntimeClient> where
	RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
	RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
	Spawner: 'static + SpawnNamed + Clone + Unpin,
{
	pub leaves: Vec<BlockInfo>,
	pub keystore: Arc<LocalKeystore>,
	pub runtime_client: Arc<RuntimeClient>,
	pub parachains_db: Arc<dyn kvdb::KeyValueDB>,
	pub availability_config: AvailabilityConfig,
	pub approval_voting_config: ApprovalVotingConfig,
	pub network_service: Arc<sc_network::NetworkService<Block, Hash>>,
	pub authority_discovery_service: AuthorityDiscoveryService,
	pub request_multiplexer: RequestMultiplexer,
	pub registry: Option<&'a Registry>,
	pub spawner: Spawner,
	pub is_collator: IsCollator,
	pub candidate_validation_config: CandidateValidationConfig,
}

/// Trait for the fn generating the overseer.
pub trait OverseerGen {
	fn generate<'a, Spawner, RuntimeClient>(&self, args: OverseerGenArgs<'a, Spawner, RuntimeClient>) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandler), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin;
}

pub struct RealOverseerGen;

impl OverseerGen for RealOverseerGen {
	fn generate<'a, Spawner, RuntimeClient>(&self,
		OverseerGenArgs {
			leaves,
			keystore,
			runtime_client,
			parachains_db,
			availability_config,
			approval_voting_config,
			network_service,
			authority_discovery_service,
			request_multiplexer,
			registry,
			spawner,
			is_collator,
			candidate_validation_config,
		} : OverseerGenArgs<'a, Spawner, RuntimeClient>
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandler), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin
	{
		use polkadot_node_subsystem_util::metrics::Metrics;

		use polkadot_availability_distribution::AvailabilityDistributionSubsystem;
		use polkadot_node_core_av_store::AvailabilityStoreSubsystem;
		use polkadot_availability_bitfield_distribution::BitfieldDistribution as BitfieldDistributionSubsystem;
		use polkadot_node_core_bitfield_signing::BitfieldSigningSubsystem;
		use polkadot_node_core_backing::CandidateBackingSubsystem;
		use polkadot_node_core_candidate_validation::CandidateValidationSubsystem;
		use polkadot_node_core_chain_api::ChainApiSubsystem;
		use polkadot_node_collation_generation::CollationGenerationSubsystem;
		use polkadot_collator_protocol::{CollatorProtocolSubsystem, ProtocolSide};
		use polkadot_network_bridge::NetworkBridge as NetworkBridgeSubsystem;
		use polkadot_node_core_provisioner::ProvisioningSubsystem as ProvisionerSubsystem;
		use polkadot_node_core_runtime_api::RuntimeApiSubsystem;
		use polkadot_statement_distribution::StatementDistribution as StatementDistributionSubsystem;
		use polkadot_availability_recovery::AvailabilityRecoverySubsystem;
		use polkadot_approval_distribution::ApprovalDistribution as ApprovalDistributionSubsystem;
		use polkadot_node_core_approval_voting::ApprovalVotingSubsystem;
		use polkadot_gossip_support::GossipSupport as GossipSupportSubsystem;

		let all_subsystems = AllSubsystems {
			availability_distribution: AvailabilityDistributionSubsystem::new(
				keystore.clone(),
				Metrics::register(registry)?,
			),
			availability_recovery: AvailabilityRecoverySubsystem::with_chunks_only(
			),
			availability_store: AvailabilityStoreSubsystem::new(
				parachains_db.clone(),
				availability_config,
				Metrics::register(registry)?,
			),
			bitfield_distribution: BitfieldDistributionSubsystem::new(
				Metrics::register(registry)?,
			),
			bitfield_signing: BitfieldSigningSubsystem::new(
				spawner.clone(),
				keystore.clone(),
				Metrics::register(registry)?,
			),
			candidate_backing: CandidateBackingSubsystem::new(
				spawner.clone(),
				keystore.clone(),
				Metrics::register(registry)?,
			),
			candidate_validation: CandidateValidationSubsystem::with_config(
				candidate_validation_config,
				Metrics::register(registry)?,
			),
			chain_api: ChainApiSubsystem::new(
				runtime_client.clone(),
				Metrics::register(registry)?,
			),
			collation_generation: CollationGenerationSubsystem::new(
				Metrics::register(registry)?,
			),
			collator_protocol: {
				let side = match is_collator {
					IsCollator::Yes(collator_pair) => ProtocolSide::Collator(
						network_service.local_peer_id().clone(),
						collator_pair,
						Metrics::register(registry)?,
					),
					IsCollator::No => ProtocolSide::Validator {
						keystore: keystore.clone(),
						eviction_policy: Default::default(),
						metrics: Metrics::register(registry)?,
					},
				};
				CollatorProtocolSubsystem::new(
					side,
				)
			},
			network_bridge: NetworkBridgeSubsystem::new(
				network_service.clone(),
				authority_discovery_service,
				request_multiplexer,
				Box::new(network_service.clone()),
				Metrics::register(registry)?,
			),
			provisioner: ProvisionerSubsystem::new(
				spawner.clone(),
				(),
				Metrics::register(registry)?,
			),
			runtime_api: RuntimeApiSubsystem::new(
				runtime_client.clone(),
				Metrics::register(registry)?,
				spawner.clone(),
			),
			statement_distribution: StatementDistributionSubsystem::new(
				keystore.clone(),
				Metrics::register(registry)?,
			),
			approval_distribution: ApprovalDistributionSubsystem::new(
				Metrics::register(registry)?,
			),
			approval_voting: ApprovalVotingSubsystem::with_config(
				approval_voting_config,
				parachains_db,
				keystore.clone(),
				Box::new(network_service.clone()),
				Metrics::register(registry)?,
			),
			gossip_support: GossipSupportSubsystem::new(
				keystore.clone(),
			),
		};

		Overseer::new(
			leaves,
			all_subsystems,
			registry,
			runtime_client.clone(),
			spawner,
		).map_err(|e| e.into())
	}
}
