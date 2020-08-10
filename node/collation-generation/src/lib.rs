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

use polkadot_node_subsystem::{
	messages::CollationGenerationMessage,
	FromOverseer,
	SpawnedSubsystem,
	Subsystem,
	SubsystemContext,
	SubsystemResult,
};
use polkadot_primitives::v1::{
	CollationGenerationConfig,
	Hash,
};

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
	async fn run<Context>(
		mut self,
		mut ctx: Context,
	)
	where
		Context: SubsystemContext<Message = CollationGenerationMessage>,
	{
		loop {
			let incoming = ctx.recv().await;
			if self.handle_incoming::<Context>(incoming).await {
				break
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
					handle_new_activations(config, &activated).await;
				}
				false
			}
			Ok(Signal(Conclude)) => {
				true
			}
			Ok(Communication { msg: CollationGenerationMessage::Initialize(config) }) => {
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
			Ok(Signal(BlockFinalized(_))) => { false }
			Err(err) => {
				log::error!(target: "collation_generation", "error receiving message from subsystem context: {:?}", err);
				true
			}
		}
	}
}

async fn handle_new_activations(config: &CollationGenerationConfig, activated: &[Hash]) {
	// follow the procedure from the guide
	unimplemented!()
}

impl<Context> Subsystem<Context> for CollationGenerationSubsystem
where
	Context: SubsystemContext<Message = CollationGenerationMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let subsystem = CollationGenerationSubsystem {
			config: None,
		};

		let future = Box::pin(subsystem.run(ctx));

		SpawnedSubsystem {
			name: "CollationGenerationSubsystem",
			future,
		}
	}
}
