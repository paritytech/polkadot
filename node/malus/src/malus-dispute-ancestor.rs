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

//! A malicious overseer that always disputes a block as
//! soon as it becomes available.
//!
//! Attention: For usage with `simnet`/`gurke` only!

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

// Filter wrapping related types.
use malus::*;

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_backing::{CandidateBackingSubsystem, Metrics as CandidateBackingMetrics};
use polkadot_node_subsystem::messages::{CandidateBackingMessage, DisputeCoordinatorMessage};
use polkadot_node_subsystem_util as util;
use polkadot_primitives::v1::CandidateReceipt;
use sp_keystore::SyncCryptoStorePtr;
use util::{metered, metrics::Metrics as _};

// Filter wrapping related types.
use malus::overseer::SubsystemSender;
use shared::*;

use std::sync::Arc;

use structopt::StructOpt;

mod shared;

/// Become Loki and throw in a dispute once in a while, for an unfinalized block.
#[derive(Clone, Debug)]
struct TrackCollations<Sender>
where
	Sender: Send,
{
	sink: metered::UnboundedMeteredSender<(Sender, CandidateReceipt)>,
}

impl<Sender> MessageInterceptor<Sender> for TrackCollations<Sender>
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
				// `DistributeCollation` is only received
				// by a _collator_, but we are a validator.
				// `CollatorProtocolMessage::DistributeCollation(ref ccr, ref pov, _)` hence
				// cannot be used.

				// Intercepting [`fn request_collation`](https://github.com/paritytech/polkadot/blob/117466aa8e471562f921a90b69a6c265cb6c656f/node/network/collator-protocol/src/validator_side/mod.rs#L736-L736)
				// is bothersome, so we wait for the seconding and
				// make that disappear, and instead craft our own message.
				msg: CandidateBackingMessage::Second(_, ccr, _),
			} => {
				// TODO FIXME avoid this, by moving all the logic in here by providing access to the spawner.
				self.sink.unbounded_send((sender.clone(), ccr)).unwrap();
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

		let (sink, source) = metered::unbounded();

		let coll = TrackCollations { sink };

		let metrics = CandidateBackingMetrics::register(registry).unwrap();

		let crypto_store_ptr = args.keystore.clone() as SyncCryptoStorePtr;

		// modify the subsystem(s) as needed:
		let all_subsystems = create_default_subsystems(args)?.replace_candidate_backing(
			// create the filtered subsystem
			FilteredSubsystem::new(
				CandidateBackingSubsystem::new(spawner.clone(), crypto_store_ptr, metrics),
				coll,
			),
		);

		let (overseer, handle) =
			Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner.clone())?;

		launch_processing_task(
			spawner.clone(),
			source,
			|(mut subsystem_sender, candidate_receipt): (_, CandidateReceipt)| async move {
				let relay_parent = candidate_receipt.descriptor().relay_parent;
				let session_index =
					util::request_session_index_for_child(relay_parent, &mut subsystem_sender)
						.await;
				let session_index = session_index.await.unwrap().unwrap();
				let candidate_hash = candidate_receipt.hash();

				tracing::warn!(
					target = MALUS,
					"Disputing candidate /w hash {} in session {:?} on relay_parent {}",
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

				subsystem_sender.send_message(msg).await;
			},
		);

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
