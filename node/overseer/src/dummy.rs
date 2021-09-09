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

//! Legacy way of defining subsystems.
//!
//! In the future, everything should be set up using the generated
//! overseer builder pattern instead.

use crate::{prometheus::Registry, AllMessages, MetricsTrait, OverseerBuilder, OverseerSignal};
use polkadot_node_subsystem_types::errors::SubsystemError;
use polkadot_overseer_gen::{FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext};

/// A dummy subsystem that implements [`Subsystem`] for all
/// types of messages. Used for tests or as a placeholder.
#[derive(Clone, Copy, Debug)]
pub struct DummySubsystem;

impl<Context> Subsystem<Context, SubsystemError> for DummySubsystem
where
	Context: SubsystemContext<
		Signal = OverseerSignal,
		Error = SubsystemError,
		AllMessages = AllMessages,
	>,
{
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<SubsystemError> {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Err(_) => return Ok(()),
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(overseer_msg) => {
						tracing::debug!(
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

/// Create an overseer with all subsystem being `DummySubsystem`.
///
/// Preferred way of initializing a dummy overseer for subsystem tests.
pub fn dummy_overseer_builder<
	'a,
	Spawner: SpawnNamed + Send + Sync + 'static,
	SupportsParachains: HeadSupportsParachains,
>(
	spawner: Spawner,
	supports_parachains: SupportsParachains,
	registry: Option<&'a Registry>,
) -> Result<
	OverseerBuilder<
		Spawner,
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
	>,
	SubsystemError,
> {
	let metrics: Metrics = <Metrics as MetricsTrait>::register(registry)?;

	let builder = Overseer::builder()
		.availability_distribution(DummySubsystem)
		.availability_recovery(DummySubsystem)
		.availability_store(DummySubsystem)
		.bitfield_distribution(DummySubsystem)
		.bitfield_signing(DummySubsystem)
		.candidate_backing(DummySubsystem)
		.candidate_validation(DummySubsystem)
		.chain_api(DummySubsystem)
		.collation_generation(DummySubsystem)
		.collator_protocol(DummySubsystem)
		.network_bridge(DummySubsystem)
		.provisioner(DummySubsystem)
		.runtime_api(DummySubsystem)
		.statement_distribution(DummySubsystem)
		.approval_distribution(DummySubsystem)
		.approval_voting(DummySubsystem)
		.gossip_support(DummySubsystem)
		.dispute_coordinator(DummySubsystem)
		.dispute_participation(DummySubsystem)
		.dispute_distribution(DummySubsystem)
		.chain_selection(DummySubsystem)
		.activation_external_listeners(Default::default())
		.span_per_active_leaf(Default::default())
		.active_leaves(Default::default())
		.known_leaves(LruCache::new(KNOWN_LEAVES_CACHE_SIZE))
		.leaves(Default::default())
		.spawner(spawner)
		.metrics(metrics)
		.supports_parachains(supports_parachains);
	Ok(builder)
}
