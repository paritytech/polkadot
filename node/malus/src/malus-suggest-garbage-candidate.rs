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

//! A malicious overseer proposing a garbage block.
//!
//! Supposed to be used with regular nodes or in conjunction
//! with [`malus-back-garbage-candidate.rs`](./malus-back-garbage-candidate.rs)
//! to simulate a coordinated attack.

#![allow(missing_docs)]

use color_eyre::eyre;
use polkadot_cli::{
	create_default_subsystems,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost, ProvideRuntimeApi, SpawnNamed,
	},
	Cli,
};

use crate::overseer::Handle;

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_candidate_validation::{CandidateValidationSubsystem, Metrics};
use polkadot_node_core_dispute_coordinator::DisputeCoordinatorSubsystem;
use polkadot_node_primitives::{BlockData, PoV, Statement, ValidationResult};
use polkadot_node_subsystem::messages::{
	CandidateBackingMessage, CandidateValidationMessage, StatementDistributionMessage,
};
use polkadot_node_subsystem_util as util;
// Filter wrapping related types.
use malus::*;
use polkadot_primitives::{
	v0::CandidateReceipt,
	v1::{CandidateCommitments, CommittedCandidateReceipt, Hash, Signed},
};
use sp_keystore::SyncCryptoStorePtr;
use util::{metered::UnboundedMeteredSender, metrics::Metrics as _};

use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use structopt::StructOpt;

use shared::*;

mod shared;
/// Replaces the seconded PoV data
/// of outgoing messages by some garbage data.
#[derive(Clone)]
struct ReplacePoVBytes<Sender>
where
	Sender: Send,
{
	keystore: SyncCryptoStorePtr,
	queue: metered::Sender<(Hash, CandidateReceipt, PoV)>,
}

impl<Sender> MsgFilter<Sender> for ReplacePoVBytes<Sender>
where
	Sender: overseer::SubsystemSender<CandidateBackingMessage> + Clone + Send + 'static,
{
	type Message = CandidateBackingMessage;

	fn filter_in(
		&self,
		sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		match msg {
			FromOverseer::Communication {
				msg: CandidateBackingMessage::Second(hash, candidate_receipt, pov),
			} => {
				self.queue.send((sender, hash, candidate_receipt, pov));
				None
				// Some(CandidateBackingMessage::Second(hash, candidate_receipt, PoV {
				// 	block_data: BlockData(MALICIOUS_POV.to_vec())
				// }))
			},
			other => Some(other),
		}
	}

	fn filter_out(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// Generates an overseer that exposes bad behavior.
struct SuggestGarbageCandidate;

impl OverseerGen for SuggestGarbageCandidate {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let spawner = args.spawner.clone();
		let leaves = args.leaves.clone();
		let runtime_client = args.runtime_client.clone();
		let registry = args.registry.clone();
		let candidate_validation_config = args.candidate_validation_config.clone();

		let (sink, source) = util::metered::unbounded();

		let metrics = Metrics::new(registry);

		let crypto_store_ptr = args.keystore.clone() as SyncCryptoStorePtr;

		let filter =
			ReplacePoVBytes { keystore, overseer: OverseerHandle::new_disconnected(), queue: sink };
		// modify the subsystem(s) as needed:
		let all_subsystems = create_default_subsystems(args)?.replace_candidate_validation(
			// create the filtered subsystem
			FilteredSubsystem::new(
				CandidateValidationSubsystem::with_config(
					candidate_validation_config,
					Metrics::register(registry)?,
				),
				filter.clone(),
			),
		);

		let (overseer, handle) =
			Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner)?;

		launch_processing_task(overseer.spawner(), source, async move {
			tracing::info!(target = MALUS, "Replacing seconded candidate pov with something else");

			let committed_candidate_receipt = CommittedCandidateReceipt {
				descriptor: candidate_receipt.descriptor(),
				commitments: CandidateCommitments::default(),
			};
			let statement = Statement::Seconded(committed_candidate_receipt);
			let compact_statement =
				CompactStatement::Seconded(candidate_receipt.descriptor().hash());
			let signed: Signed<Statement, CompactStatement> = todo!();
			subsystem_sender
				.send_message(StatementDistributionMessage::Share(hash, signed_statement))
		});

		Ok((overseer, handle))
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = Cli::from_args();
	assert_matches::assert_matches!(cli.subcommand, None);
	polkadot_cli::run_node(cli, SuggestGarbageCandidate)?;
	Ok(())
}
