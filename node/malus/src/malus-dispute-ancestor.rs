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

//! A malicious overseer that always disputes an ancestor block.
//!
//! Attention: For usage with `simnet`/`gurke` only!

#![allow(missing_docs)]

use color_eyre::eyre;
use polkadot_cli::{Cli, create_default_subsystems, service::{AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost, ProvideRuntimeApi, SpawnNamed, polkadot_runtime::Session}};

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_candidate_validation::{CandidateValidationSubsystem, Metrics};
use polkadot_node_subsystem::messages::{CandidateValidationMessage, DisputeCoordinatorMessage, DisputeDistributionMessage};
use polkadot_node_subsystem_util::metrics::Metrics as _;
use polkadot_node_subsystem_util as util;

// Filter wrapping related types.
use malus::{self::*, overseer::SubsystemSender};

use std::{collections::{HashMap, VecDeque}, sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}};

use structopt::StructOpt;

/// Become Loki and throw in a dispute once in a while, for an unfinalized block.
#[derive(Clone, Default, Debug)]
struct Loki(Arc<Mutex<VecDeque<(SessionIndex, CandidateReceipt)>>>);

impl MsgFilter for Loki {
	type Message = DisputeCoordinatorMessage;

	fn filter_in(&self, msg: FromOverseer<Self::Message>) -> Option<FromOverseer<Self::Message>>
	{
		match msg {
			FromOverseer::Communication{
				msg: AllMessages::CollatorProtocol(
					CollatorProtocolMessage::DistributeCollation(ref ccr, ref pov, _),
				),
			} @ msg => {
				let guard = self.0.lock().unwrap();
				guard.queue.extend(ccr.clone());
				Some(msg)
			},
			msg => Some(msg),
		}
	}

	fn filter_out(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

fn phony_dispute(sender: impl SubsystemSender,
	session_index: Session,
	candidate_hash: CandidateHash,
	candidate: CandidateReceipt,
) {
	let msg = DisputeCoordinatorMessage::IssueLocalStatement(
		session_index,
		candidate_hash,
		candidate,
		false,
	);

	sender.send(msg).await.unwrap();
}

/// Generates an overseer that disputes an ancestor.
struct MalusDisputeAncestor;

impl OverseerGen for MalusDisputeAncestor {
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

		// modify the subsystem(s) as needed:
		let all_subsystems = create_default_subsystems(args)?.replace_candidate_validation(
			// create the filtered subsystem
			FilteredSubsystem::new(
				CandidateValidationSubsystem::with_config(
					candidate_validation_config,
					Metrics::register(registry)?,
				),
				Skippy::default(),
			),
		);

		let (overseer, handle) = Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner)
			.map_err(|e| e.into())?;

		{
			let handle = handle.clone();
			overseer.spawner().spawn("nemesis", async move {
				while let Some((relay_parent, candidate_receipt)) = rx.next() {
					let session_index = util::request_session_index_for_child(relay_parent).await?;
					let candidate_hash = candidate_receipt.hash();
					tracing::warn!(target=MALUS, "Disputing candidate /w hash {} in session {} on relay_parent {}",
						candidate_hash,
						session_index,
						relay_parent,
					);

					let msg = DisputeCoordinatorMessage::IssueLocalStatement(
						session_index,
						candidate_hash,
						candidate,
						false,
					);

					handle.send(msg).await.unwrap();
				}
				Ok(())
			});
		}

		Ok((overseer, handle))
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = Cli::from_args();
	assert_matches::assert_matches!(cli.subcommand, None);
	polkadot_cli::run_node(cli, BehaveMaleficient)?;
	Ok(())
}
