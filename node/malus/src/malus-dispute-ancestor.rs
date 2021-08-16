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

//! A malicious overseer that always a block as soon as it becomes available.
//!
//! Attention: For usage with `simnet`/`gurke` only!

#![allow(missing_docs)]

use color_eyre::eyre;
use polkadot_cli::{
	create_default_subsystems,
	service::{
		polkadot_runtime::Session, AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error,
		HeaderBackend, Overseer, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost,
		ProvideRuntimeApi, SpawnNamed,
	},
	Cli,
};

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_backing::{CandidateBackingSubsystem, Metrics};
use polkadot_node_core_candidate_validation::CandidateValidationSubsystem;
use polkadot_node_core_dispute_coordinator::DisputeCoordinator;
use polkadot_node_subsystem::messages::{
	CandidateBackingMessage, CandidateValidationMessage, DisputeCoordinatorMessage,
	DisputeDistributionMessage,
};
use polkadot_node_subsystem_types::messages::CollatorProtocolMessage;
use polkadot_node_subsystem_util as util;
use polkadot_primitives::v1::{CandidateHash, CandidateReceipt, SessionIndex};
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use std::pin::Pin;
use util::{metered::UnboundedMeteredSender, metrics::Metrics as _};

// Filter wrapping related types.
use malus::{overseer::SubsystemSender, *};

use std::{
	collections::{HashMap, VecDeque},
	sync::{
		atomic::{AtomicUsize, Ordering},
		Arc, Mutex,
	},
};

use structopt::StructOpt;

/// Become Loki and throw in a dispute once in a while, for an unfinalized block.
#[derive(Clone, Debug)]
struct TrackCollations {
	sink: UnboundedMeteredSender<CandidateReceipt>,
}

impl MsgFilter for TrackCollations {
	type Message = CandidateBackingMessage;

	fn filter_in(&self, msg: FromOverseer<Self::Message>) -> Option<FromOverseer<Self::Message>> {
		match msg {
			FromOverseer::Communication {
				// `DistributeCollation` is only received
				// by a _collator_, but we are a validator.
				// `CollatorProtocolMessage::DistributeCollation(ref ccr, ref pov, _)` hence
				// cannot be used.

				// Intercepting [`fn request_collation`](https://github.com/paritytech/polkadot/blob/117466aa8e471562f921a90b69a6c265cb6c656f/node/network/collator-protocol/src/validator_side/mod.rs#L736-L736)
				// is bothersome, so we skip wait for the seconding and
				// make that disappear.
				msg: CandidateBackingMessage::Second(_, ccr, _),
			} => {
				self.sink.send(ccr);
				None
			},
			msg => Some(msg),
		}
	}

	fn filter_out(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// Generates an overseer that disputes every ancestor.
struct DisputeEverything;

impl OverseerGen for DisputeEverything {
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

		let coll = TrackCollations { sink };

		let metrics = Metrics::new(registry);

		let crypto_store_ptr = args.keystore.clone() as SyncCryptoStorePtr;

		// modify the subsystem(s) as needed:
		let all_subsystems = create_default_subsystems(args)?.replace_candidate_backing(
			// create the filtered subsystem
			FilteredSubsystem::new(
				CandidateBackingSubsystem::new(spawner, crypto_store_ptr, metrics),
				TrackCollations,
			),
		);

		let (overseer, handle) =
			Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner)
				.map_err(|e| e.into())?;
		{
			let handle = handle.clone();
			let spawner = overseer.spawner();
			spawner.spawn(
				"nemesis",
				Pin::new(async move {
					source.for_each(move |candidate_receipt| {
						spawner.spawn(
							"nemesis-inner",
							Box::pin(async move {
								let relay_parent = candidate_receipt.descriptor().relay_parent;
								let session_index =
									util::request_session_index_for_child(relay_parent)
										.await
										.unwrap();
								let candidate_hash = candidate_receipt.hash();

								tracing::warn!(target=MALUS, "Disputing candidate /w hash {} in session {} on relay_parent {}",
								candidate_hash,
								session_index,
								relay_parent,
							);

								// consider adding a delay here
								// Delay::new(Duration::from_secs(12)).await;

								// 😈
								let msg = DisputeCoordinatorMessage::IssueLocalStatement(
									session_index,
									candidate_hash,
									candidate_receipt,
									false,
								);

								handle.send(msg).await.unwrap();
							}),
						);
					});
					Ok(())
				}),
			);
		}

		Ok((overseer, handle))
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = Cli::from_args();
	assert_matches::assert_matches!(cli.subcommand, None);
	polkadot_cli::run_node(cli, DisputeEverything)?;
	Ok(())
}
