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

//! The collation generation subsystem is the interface between polkadot and the collators.

#![deny(missing_docs)]

use futures::channel::oneshot;
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, CollationGenerationMessage, CollatorProtocolMessage},
	FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	self as util, request_availability_cores_ctx, request_global_validation_data_ctx,
	request_local_validation_data_ctx,
};
use polkadot_primitives::v1::{
	collator_signature_payload, validation_data_hash, CandidateCommitments, CandidateDescriptor,
	CollationGenerationConfig, CommittedCandidateReceipt, CoreState, Hash, OccupiedCoreAssumption,
};
use sp_core::crypto::Pair;

/// Collation Generation Subsystem
pub struct CollationGenerationSubsystem {
	config: Option<CollationGenerationConfig>,
}

impl CollationGenerationSubsystem {
	/// Run this subsystem
	///
	/// Conceptually, this is very simple: it just loops forever.
	///
	/// - On incoming overseer messages, it starts or stops jobs as appropriate.
	/// - On other incoming messages, if they can be converted into Job::ToJob and
	///   include a hash, then they're forwarded to the appropriate individual job.
	/// - On outgoing messages from the jobs, it forwards them to the overseer.
	///
	/// If `err_tx` is not `None`, errors are forwarded onto that channel as they occur.
	/// Otherwise, most are logged and then discarded.
	async fn run<Context>(mut self, mut ctx: Context)
	where
		Context: SubsystemContext<Message = CollationGenerationMessage>,
	{
		loop {
			let incoming = ctx.recv().await;
			if self.handle_incoming::<Context>(incoming, &mut ctx).await {
				break;
			}
		}
	}

	// handle an incoming message. return true if we should break afterwards.
	// note: this doesn't strictly need to be a separate function; it's more an administrative function
	// so that we don't clutter the run loop. It could in principle be inlined directly into there.
	// it should hopefully therefore be ok that it's an async function mutably borrowing self.
	async fn handle_incoming<Context>(
		&mut self,
		incoming: SubsystemResult<FromOverseer<Context::Message>>,
		ctx: &mut Context,
	) -> bool
	where
		Context: SubsystemContext<Message = CollationGenerationMessage>,
	{
		use polkadot_node_subsystem::ActiveLeavesUpdate;
		use polkadot_node_subsystem::FromOverseer::{Communication, Signal};
		use polkadot_node_subsystem::OverseerSignal::{ActiveLeaves, BlockFinalized, Conclude};

		match incoming {
			Ok(Signal(ActiveLeaves(ActiveLeavesUpdate { activated, .. }))) => {
				// follow the procedure from the guide
				if let Some(ref config) = self.config {
					if let Err(err) = handle_new_activations(config, &activated, ctx).await {
						log::warn!(target: "collation_generation", "failed to handle new activations: {:?}", err);
						return true;
					};
				}
				false
			}
			Ok(Signal(Conclude)) => true,
			Ok(Communication {
				msg: CollationGenerationMessage::Initialize(config),
			}) => {
				// REVIEW: what happens if someone sends two initializaiton messages:
				// panic, replace, ignore, abort?
				// for now, we won't panic, but we'll break the run loop
				if self.config.is_some() {
					log::warn!(target: "collation_generation", "double initialization");
					true
				} else {
					self.config = Some(config);
					false
				}
			}
			Ok(Signal(BlockFinalized(_))) => false,
			Err(err) => {
				log::error!(target: "collation_generation", "error receiving message from subsystem context: {:?}", err);
				true
			}
		}
	}
}

impl<Context> Subsystem<Context> for CollationGenerationSubsystem
where
	Context: SubsystemContext<Message = CollationGenerationMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let subsystem = CollationGenerationSubsystem { config: None };

		let future = Box::pin(subsystem.run(ctx));

		SpawnedSubsystem {
			name: "CollationGenerationSubsystem",
			future,
		}
	}
}

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Subsystem(SubsystemError),
	#[from]
	OneshotRecv(oneshot::Canceled),
	#[from]
	Runtime(RuntimeApiError),
	#[from]
	Util(util::Error),
}

type Result<T> = std::result::Result<T, Error>;

async fn handle_new_activations<Context: SubsystemContext>(
	config: &CollationGenerationConfig,
	activated: &[Hash],
	ctx: &mut Context,
) -> Result<()> {
	// follow the procedure from the guide:
	// https://w3f.github.io/parachain-implementers-guide/node/collators/collation-generation.html

	for relay_parent in activated.iter().copied() {
		let global_validation_data = request_global_validation_data_ctx(relay_parent, ctx)
			.await?
			.await??;

		let availability_cores = request_availability_cores_ctx(relay_parent, ctx)
			.await?
			.await??;

		// REVIEW: this section is largely cribbed from the provisioner. Is this a safe assumption to make?
		// If yes: TODO: we should abstract away the duplication between these sections
		for core in availability_cores {
			let (scheduled_core, assumption) = match core {
				CoreState::Scheduled(scheduled_core) => {
					(scheduled_core, OccupiedCoreAssumption::Free)
				}
				CoreState::Occupied(occupied_core) => {
					if let Some(scheduled_core) = occupied_core.next_up_on_available {
						(scheduled_core, OccupiedCoreAssumption::Included)
					} else {
						// REVIEW: the guide doesn't mention it, but should we also handle the timed out case?
						continue;
					}
				}
				_ => continue,
			};

			let local_validation_data = match request_local_validation_data_ctx(
				relay_parent,
				scheduled_core.para_id,
				assumption,
				ctx,
			)
			.await?
			.await??
			{
				Some(local_validation_data) => local_validation_data,
				None => continue,
			};

			let validation_data_hash =
				validation_data_hash(&global_validation_data, &local_validation_data);

			let collation = (config.collator)(&global_validation_data, &local_validation_data).await;

			let pov_hash = collation.proof_of_validity.hash();

			let ccr = CommittedCandidateReceipt {
				commitments: CandidateCommitments {
					upward_messages: collation.upward_messages,
					head_data: collation.head_data,
					..Default::default()
				},
				descriptor: CandidateDescriptor {
					signature: config.key.sign(&collator_signature_payload(
						&relay_parent,
						&scheduled_core.para_id,
						&validation_data_hash,
						&pov_hash,
					)),
					para_id: scheduled_core.para_id,
					relay_parent,
					collator: config.key.public(),
					validation_data_hash,
					pov_hash,
				},
			};

			ctx.send_message(AllMessages::CollatorProtocol(
				CollatorProtocolMessage::DistributeCollation(ccr, collation.proof_of_validity),
			)).await?;
		}
	}

	Ok(())
}
