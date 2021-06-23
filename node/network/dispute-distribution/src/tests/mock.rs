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
//

//! Mock data and utility functions for unit tests in this subsystem.

use std::sync::Arc;

use async_trait::async_trait;

use polkadot_node_network_protocol::{PeerId, authority_discovery::AuthorityDiscovery};
use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_keyring::{Sr25519Keyring};
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};

use polkadot_node_primitives::{DisputeMessage, SignedDisputeStatement};
use polkadot_primitives::v1::{
	CandidateDescriptor, CandidateHash, CandidateReceipt, Hash,
	SessionIndex, SessionInfo, ValidatorId, ValidatorIndex
};


pub const MOCK_SESSION_INDEX: SessionIndex = 1;
pub const MOCK_VALIDATORS: [Sr25519Keyring; 2] = [
	Sr25519Keyring::Ferdie,
	Sr25519Keyring::Alice,
];

pub const FERDIE_INDEX: ValidatorIndex = ValidatorIndex(0);
pub const ALICE_INDEX: ValidatorIndex = ValidatorIndex(1);

pub fn make_session_info() -> SessionInfo {
	SessionInfo {
		validators: MOCK_VALIDATORS.iter().map(|k| k.public().into()).collect(),
		discovery_keys: MOCK_VALIDATORS.iter().map(|k| k.public().into()).collect(),
		..Default::default()
	}
}

pub fn make_candidate_receipt(relay_parent: Hash) -> CandidateReceipt {
	CandidateReceipt {
		descriptor: CandidateDescriptor {
			relay_parent,
			..Default::default()
		},
		commitments_hash: Hash::random(),
	}
}

pub async fn make_explicit_signed(
	validator: Sr25519Keyring,
	candidate_hash: CandidateHash,
	valid: bool
) -> SignedDisputeStatement {
	let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
	SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&validator.to_seed()),
	)
	.expect("Insert key into keystore");

	SignedDisputeStatement::sign_explicit(
		&keystore,
		valid,
		candidate_hash,
		MOCK_SESSION_INDEX,
		validator.public().into(),
	)
	.await
	.expect("Keystore should be fine.")
	.expect("Signing should work.")
}


pub async fn make_dispute_message(
	candidate: CandidateReceipt,
	valid_validator: ValidatorIndex,
	invalid_validator: ValidatorIndex,
) -> DisputeMessage {
	let candidate_hash = candidate.hash();
	let valid_vote = 
		make_explicit_signed(MOCK_VALIDATORS[valid_validator.0 as usize], candidate_hash, true).await;
	let invalid_vote =
		make_explicit_signed(MOCK_VALIDATORS[invalid_validator.0 as usize], candidate_hash, false).await;
	DisputeMessage::from_signed_statements(
		valid_vote,
		valid_validator,
		invalid_vote,
		invalid_validator,
		candidate,
		&make_session_info(),
	)
	.expect("DisputeMessage construction should work.")
}

/// Dummy `AuthorityDiscovery` service.
#[derive(Debug, Clone)]
pub struct MockAuthorityDiscovery {
	peer_ids: Vec<PeerId>
}

impl MockAuthorityDiscovery {
	pub fn new() -> Self {
		Self {
			peer_ids: vec![
				PeerId::random(),
				PeerId::random(),
			]
		}
	}

	pub fn get_peer_id_by_index(&self, index: ValidatorIndex) -> PeerId {
		self.peer_ids[index.0 as usize]
	}
}

#[async_trait]
impl AuthorityDiscovery for MockAuthorityDiscovery {
	async fn get_addresses_by_authority_id(&mut self, authority: polkadot_primitives::v1::AuthorityDiscoveryId)
		-> Option<Vec<sc_network::Multiaddr>> {
			panic!("Not implemented");
	}

	async fn get_authority_id_by_peer_id(&mut self, peer_id: polkadot_node_network_protocol::PeerId)
		-> Option<polkadot_primitives::v1::AuthorityDiscoveryId> {
		for (i, p) in self.peer_ids.iter().enumerate() {
			if p == &peer_id {
				return Some(MOCK_VALIDATORS[i].public().into())
			}
		}
		None
	}
}
