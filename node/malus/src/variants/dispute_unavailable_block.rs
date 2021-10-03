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
//! as it is observed.
//!
//! Attention: For usage with `simnet`/`gurke` only!

#![allow(missing_docs)]

use polkadot_cli::{
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost, ProvideRuntimeApi, SpawnNamed,
		OverseerConnector,
	},
};

// Filter wrapping related types.
use crate::{interceptor::*, shared::*};
use polkadot_node_subsystem::overseer::SubsystemSender;

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_core_backing::CandidateBackingSubsystem;
use polkadot_node_subsystem::messages::{CandidateBackingMessage, DisputeCoordinatorMessage};
use polkadot_node_subsystem_util as util;
use polkadot_primitives::v1::CandidateReceipt;
use sp_keystore::SyncCryptoStorePtr;
use util::metered;

use std::sync::Arc;

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

	fn intercept_incoming(
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
				self.sink.unbounded_send((sender.clone(), ccr)).unwrap();
				None
			},
			msg => Some(msg),
		}
	}

	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// Generates an overseer that disputes every ancestor.
pub(crate) struct DisputeUnavailable;

impl OverseerGen for DisputeUnavailable {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let spawner = args.spawner.clone();
		let (sink, source) = metered::unbounded();
		let track_collations = TrackCollations { sink };

		let crypto_store_ptr = args.keystore.clone() as SyncCryptoStorePtr;
		let spawner2 = spawner.clone();
		let result = prepared_overseer_builder(args)?.replace_candidate_backing(|cb|
			InterceptedSubsystem::new(
				CandidateBackingSubsystem::new(spawner2, crypto_store_ptr, cb.params.metrics),
				track_collations,
			)
		)
			.build_with_connector(connector)
			.map_err(|e| e.into());

		launch_processing_task(
			&spawner,
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
					"Disputing unvailable block with candidate /w hash {} in session {:?} on relay_parent {}",
					candidate_hash,
					session_index,
					relay_parent,
				);

				// no delay, dispute right away, before it becomes available

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

		result
	}
}
