// Copyright 2021 Parity Technologies (UK) Ltd.
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
use malus::overseer::SubsystemSender;
use polkadot_cli::{
	create_default_subsystems,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost, ProvideRuntimeApi, SpawnNamed,
	},
	Cli,
};

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_backing::{CandidateBackingSubsystem, Metrics};
use polkadot_node_primitives::{BlockData, PoV, Statement};
use polkadot_node_subsystem::messages::{CandidateBackingMessage, StatementDistributionMessage};
use polkadot_node_subsystem_util as util;
// Filter wrapping related types.
use malus::*;
use polkadot_primitives::v1::{
	CandidateCommitments, CandidateReceipt, CommittedCandidateReceipt, CompactStatement, Hash,
	Signed,
};
use sp_keystore::SyncCryptoStorePtr;
use util::{metered, metrics::Metrics as _};

use std::sync::Arc;

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
	queue: metered::UnboundedMeteredSender<(Sender, Hash, CandidateReceipt)>,
}

impl<Sender> MessageInterceptor<Sender> for ReplacePoVBytes<Sender>
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
				msg: CandidateBackingMessage::Second(hash, candidate_receipt, _pov),
			} => {
				self.queue
					.unbounded_send((sender.clone(), hash, candidate_receipt.clone()))
					.unwrap();

				Some(FromOverseer::Communication {
					msg: CandidateBackingMessage::Second(
						hash,
						candidate_receipt,
						PoV { block_data: BlockData(MALICIOUS_POV.to_vec()) },
					),
				})
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

		let (sink, source) = metered::unbounded();

		let keystore = args.keystore.clone() as SyncCryptoStorePtr;

		let filter = ReplacePoVBytes { keystore: keystore.clone(), queue: sink };
		// modify the subsystem(s) as needed:
		let all_subsystems = create_default_subsystems(args)?.replace_candidate_backing(
			// create the filtered subsystem
			FilteredSubsystem::new(
				CandidateBackingSubsystem::new(
					spawner.clone(),
					keystore.clone(),
					Metrics::register(registry)?,
				),
				filter.clone(),
			),
		);

		let (overseer, handle) =
			Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner.clone())?;

		launch_processing_task(
			spawner,
			source,
			move |(mut subsystem_sender, hash, candidate_receipt): (_, Hash, CandidateReceipt)| {
				let keystore = keystore.clone();
				async move {
					tracing::info!(
						target = MALUS,
						"Replacing seconded candidate pov with something else"
					);

					let committed_candidate_receipt = CommittedCandidateReceipt {
						descriptor: candidate_receipt.descriptor.clone(),
						commitments: CandidateCommitments::default(),
					};

					let statement = Statement::Seconded(committed_candidate_receipt);

					if let Ok(validator) =
						util::Validator::new(hash, keystore.clone(), &mut subsystem_sender).await
					{
						let signed_statement: Signed<Statement, CompactStatement> = validator
							.sign(keystore, statement)
							.await
							.expect("Signing works. qed")
							.expect("Something must come out of this. qed");

						subsystem_sender
							.send_message(StatementDistributionMessage::Share(
								hash,
								signed_statement,
							))
							.await;
					} else {
						tracing::info!("We are not a validator. Not siging anything.");
					}
				}
			},
		);

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
