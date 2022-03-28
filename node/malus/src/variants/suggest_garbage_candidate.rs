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

//! A malicious node that stores bogus availability chunks, preventing others from
//! doing approval voting. This should lead to disputes depending if the validator
//! has fetched a malicious chunk.
//!
//! Attention: For usage with `zombienet` only!

#![allow(missing_docs)]

use polkadot_cli::{
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerConnector, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost,
		ProvideRuntimeApi, SpawnNamed,
	},
};
use polkadot_node_primitives::{AvailableData, BlockData, PoV};
use polkadot_primitives::v2::{CandidateCommitments, CandidateDescriptor, CandidateHash, Hash};

// Filter wrapping related types.
use crate::{
	interceptor::*,
	shared::{MALICIOUS_POV, MALUS},
};

// Import extra types relevant to the particular
// subsystem.
use polkadot_node_subsystem::messages::{AvailabilityStoreMessage, CandidateBackingMessage};
use polkadot_primitives::v2::{CandidateReceipt, CommittedCandidateReceipt};

use std::sync::{Arc, Mutex};

struct NotedCandidate {
	candidate: CandidateReceipt,
	relay_parent: Hash,
}

#[derive(Default)]
struct Inner {
	map: std::collections::HashMap<CandidateHash, NotedCandidate>,
}

/// Replace outgoing approval messages with disputes.
#[derive(Clone)]
struct NoteCandidate {
	inner: Arc<Mutex<Inner>>,
}

impl<Sender> MessageInterceptor<Sender> for NoteCandidate
where
	Sender: overseer::SubsystemSender<CandidateBackingMessage> + Clone + Send + 'static,
{
	type Message = CandidateBackingMessage;

	/// Cache and forward `CandidateBackingMessage::Second`. This is called by collator protocol (validator side).
	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		match msg {
			FromOverseer::Communication {
				msg: CandidateBackingMessage::Second(relay_parent, candidate, pov),
			} => {
				let mut candidate_cache = self.inner.lock().unwrap();
				candidate_cache.map.insert(
					candidate.hash(),
					NotedCandidate {
						candidate: candidate.clone(),
						relay_parent: relay_parent.clone(),
					},
				);
				gum::info!(
					target: MALUS,
					candidate_hash = ?candidate.hash(),
					"Cached candidate"
				);
				let message = FromOverseer::Communication {
					msg: CandidateBackingMessage::Second(relay_parent, candidate, pov),
				};

				Some(message)
			},
			FromOverseer::Communication { msg } => Some(FromOverseer::Communication { msg }),
			FromOverseer::Signal(signal) => Some(FromOverseer::Signal(signal)),
		}
	}

	/// The goal of this is to make the malicious candidate available to other honest validators. The backing
	/// subsystem sends `AvailabilityStore::StoreAvailableData` via `make_pov_available()` once a candidate is
	/// backed.
	/// This implementation intercepts the outgoing `AvailabilityStore::StoreAvailableData` message and mangles
	/// it before forwarding it to the overseer(then `av-store`). More specifically, it replaces the candidate
	/// (pov, persistent validation data and collator signagure) while computing the correct erasure root hash
	/// from original candidate.
	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		match msg {
			AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreAvailableData {
				candidate_hash,
				n_validators,
				available_data,
				tx,
			}) => {
				let pov = Arc::new(PoV { block_data: BlockData(MALICIOUS_POV.into()) });
				let malicious_available_data = AvailableData {
					pov: pov.clone(),
					validation_data: available_data.validation_data.clone(),
				};

				let pov_hash = pov.hash();
				let validation_data_hash = malicious_available_data.validation_data.hash();

				let inner = self.inner.lock().unwrap();
				let cache = inner.map.get(&candidate_hash).unwrap();
				let relay_parent = cache.relay_parent.clone();
				let candidate_cache = cache.candidate.clone();
				let candidate_descriptor = candidate_cache.descriptor();
				let validation_code_hash = candidate_cache.descriptor().validation_code_hash;

				let erasure_root = {
					let chunks =
						erasure::obtain_chunks_v1(n_validators as usize, &available_data).unwrap();

					let branches = erasure::branches(chunks.as_ref());
					branches.root()
				};

				let (collator_id, collator_signature) = {
					use polkadot_primitives::v2::CollatorPair;
					use sp_core::crypto::Pair;

					let collator_pair = CollatorPair::generate().0;
					let signature_payload = polkadot_primitives::v2::collator_signature_payload(
						&relay_parent,
						&candidate_cache.descriptor().para_id,
						&validation_data_hash,
						&pov_hash,
						&validation_code_hash,
					);

					(collator_pair.public(), collator_pair.sign(&signature_payload))
				};
				// Use fake candidate validation to get commitments.
				let malicious_commitments = CandidateCommitments {
					upward_messages: Vec::new(),
					horizontal_messages: Vec::new(),
					new_validation_code: None,
					head_data: vec![1, 2, 3, 4, 5].into(),
					processed_downward_messages: 0,
					hrmp_watermark: available_data.validation_data.relay_parent_number,
				};
				let malicious_candidate = CommittedCandidateReceipt {
					descriptor: CandidateDescriptor {
						para_id: candidate_cache.descriptor().para_id,
						relay_parent,
						collator: collator_id,
						persisted_validation_data_hash: validation_data_hash,
						pov_hash,
						erasure_root,
						signature: collator_signature,
						para_head: malicious_commitments.head_data.hash(),
						validation_code_hash,
					},
					commitments: malicious_commitments.clone(),
				};
				let malicious_candidate_hash = malicious_candidate.hash();

				gum::info!(target: MALUS, ?candidate_hash, "Replacing candidate availability data");

				// We need to use the same candidate hash for storing the availability data, such that
				// other's can fetch it.
				Some(AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreAvailableData {
					candidate_hash,
					n_validators,
					available_data: malicious_available_data,
					tx,
				}))
			},
			msg => Some(msg),
		}
	}
}

/// Garbage candidate implementation wrapper which implements `OverseerGen` glue.
pub(crate) struct BackGarbageCandidateWrapper;

impl OverseerGen for BackGarbageCandidateWrapper {
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
		let inner = Inner { map: std::collections::HashMap::new() };
		let inner_mut = Arc::new(Mutex::new(inner));
		let note_candidate = NoteCandidate { inner: inner_mut.clone() };

		prepared_overseer_builder(args)?
			.replace_candidate_backing(move |cb| InterceptedSubsystem::new(cb, note_candidate))
			.build_with_connector(connector)
			.map_err(|e| e.into())
	}
}
