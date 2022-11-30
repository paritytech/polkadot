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

use crate::{
	prometheus::Registry, HeadSupportsParachains, InitializedOverseerBuilder, MetricsTrait,
	Overseer, OverseerMetrics, OverseerSignal, OverseerSubsystemContext, SpawnGlue,
	KNOWN_LEAVES_CACHE_SIZE,
};
use lru::LruCache;
use orchestra::{FromOrchestra, SpawnedSubsystem, Subsystem, SubsystemContext};
use polkadot_node_subsystem_types::{errors::SubsystemError, messages::*};
/// A dummy subsystem that implements [`Subsystem`] for all
/// types of messages. Used for tests or as a placeholder.
#[derive(Clone, Copy, Debug)]
pub struct DummySubsystem;

impl<Context> Subsystem<Context, SubsystemError> for DummySubsystem
where
	Context: SubsystemContext<Signal = OverseerSignal, Error = SubsystemError>,
{
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<SubsystemError> {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Err(_) => return Ok(()),
					Ok(FromOrchestra::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(overseer_msg) => {
						gum::debug!(
							target: "dummy-subsystem",
							"Discarding a message sent from overseer {:?}",
							overseer_msg
						);
						continue
					},
				}
			}
		});

		SpawnedSubsystem { name: "dummy-subsystem", future }
	}
}

/// Create an overseer with all subsystem being `Sub`.
///
/// Preferred way of initializing a dummy overseer for subsystem tests.
pub fn dummy_overseer_builder<Spawner, SupportsParachains>(
	spawner: Spawner,
	supports_parachains: SupportsParachains,
	registry: Option<&Registry>,
) -> Result<
	InitializedOverseerBuilder<
		SpawnGlue<Spawner>,
		SupportsParachains,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
		DummySubsystem,
	>,
	SubsystemError,
>
where
	SpawnGlue<Spawner>: orchestra::Spawner + 'static,
	SupportsParachains: HeadSupportsParachains,
{
	one_for_all_overseer_builder(spawner, supports_parachains, DummySubsystem, registry)
}

/// Create an overseer with all subsystem being `Sub`.
pub fn one_for_all_overseer_builder<Spawner, SupportsParachains, Sub>(
	spawner: Spawner,
	supports_parachains: SupportsParachains,
	subsystem: Sub,
	registry: Option<&Registry>,
) -> Result<
	InitializedOverseerBuilder<
		SpawnGlue<Spawner>,
		SupportsParachains,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
		Sub,
	>,
	SubsystemError,
>
where
	SpawnGlue<Spawner>: orchestra::Spawner + 'static,
	SupportsParachains: HeadSupportsParachains,
	Sub: Clone
		+ Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<AvailabilityRecoveryMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<CandidateBackingMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<CandidateValidationMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<ChainApiMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<CollationGenerationMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<CollatorProtocolMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<NetworkBridgeRxMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<NetworkBridgeTxMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<ProvisionerMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<RuntimeApiMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<StatementDistributionMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<ApprovalDistributionMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<ApprovalVotingMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<GossipSupportMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<DisputeCoordinatorMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<DisputeDistributionMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<ChainSelectionMessage>, SubsystemError>
		+ Subsystem<OverseerSubsystemContext<PvfCheckerMessage>, SubsystemError>,
{
	let metrics = <OverseerMetrics as MetricsTrait>::register(registry)?;

	let builder = Overseer::builder()
		.availability_distribution(subsystem.clone())
		.availability_recovery(subsystem.clone())
		.availability_store(subsystem.clone())
		.bitfield_distribution(subsystem.clone())
		.bitfield_signing(subsystem.clone())
		.candidate_backing(subsystem.clone())
		.candidate_validation(subsystem.clone())
		.pvf_checker(subsystem.clone())
		.chain_api(subsystem.clone())
		.collation_generation(subsystem.clone())
		.collator_protocol(subsystem.clone())
		.network_bridge_tx(subsystem.clone())
		.network_bridge_rx(subsystem.clone())
		.provisioner(subsystem.clone())
		.runtime_api(subsystem.clone())
		.statement_distribution(subsystem.clone())
		.approval_distribution(subsystem.clone())
		.approval_voting(subsystem.clone())
		.gossip_support(subsystem.clone())
		.dispute_coordinator(subsystem.clone())
		.dispute_distribution(subsystem.clone())
		.chain_selection(subsystem)
		.activation_external_listeners(Default::default())
		.span_per_active_leaf(Default::default())
		.active_leaves(Default::default())
		.known_leaves(LruCache::new(KNOWN_LEAVES_CACHE_SIZE))
		.leaves(Default::default())
		.spawner(SpawnGlue(spawner))
		.metrics(metrics)
		.supports_parachains(supports_parachains);
	Ok(builder)
}
