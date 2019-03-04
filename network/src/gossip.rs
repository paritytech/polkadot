// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Gossip messages and the message validator

use substrate_network::consensus_gossip::{
	self as network_gossip, ValidationResult as GossipValidationResult,
};
use polkadot_consensus::SignedStatement;
use polkadot_primitives::{Hash, SessionKey};
use codec::Decode;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::NetworkService;

/// The engine ID of the polkadot attestation system.
pub const POLKADOT_ENGINE_ID: substrate_network::ConsensusEngineId = [b'd', b'o', b't', b'1'];

/// A gossip message.
#[derive(Encode, Decode, Clone)]
pub(crate) struct GossipMessage {
	/// The relay chain parent hash.
	pub(crate) relay_parent: Hash,
	/// The signed statement being gossipped.
	pub(crate) statement: SignedStatement,
}

/// whether a block is known.
pub enum Known {
	/// The block is a known leaf.
	Leaf,
	/// The block is known to be old.
	Old,
	/// The block is known to be bad.
	Bad,
}

/// An oracle for known blocks.
pub trait KnownOracle: Send + Sync {
	/// whether a block is known. If it's not, returns `None`.
	fn is_known(&self, block_hash: &Hash) -> Option<Known>;
}

impl<F> KnownOracle for F where F: Fn(&Hash) -> Option<Known> + Send + Sync {
	fn is_known(&self, block_hash: &Hash) -> Option<Known> {
		(self)(block_hash)
	}
}

/// Register a gossip validator on the network service.
///
/// This returns a `RegisteredMessageValidator`
// NOTE: since RegisteredMessageValidator is meant to be a type-safe proof
// that we've actually done the registration, this should be the only way
// to construct it outside of tests.
pub fn register_validator<O: KnownOracle + 'static>(
	service: &NetworkService,
	oracle: O,
) -> RegisteredMessageValidator {
	let validator = Arc::new(MessageValidator {
		live_consensus: RwLock::new(HashMap::new()),
		oracle,
	});

	let gossip_side = validator.clone();
	service.with_gossip(|gossip, _| gossip.register_validator(POLKADOT_ENGINE_ID, gossip_side));

	RegisteredMessageValidator { inner: validator as _ }
}

/// A registered message validator.
///
/// Create this using `register_validator`.
#[derive(Clone)]
pub struct RegisteredMessageValidator {
	inner: Arc<MessageValidator<KnownOracle>>,
}

impl RegisteredMessageValidator {
	/// Note a live consensus session. This must be removed later with
	/// `remove_consensus`.
	pub(crate) fn note_consensus(&self, relay_parent: Hash, validation: MessageValidationData) {
		self.inner.live_consensus.write().insert(relay_parent, validation);
	}

	/// Remove a live consensus session when it is no longer live.
	pub(crate) fn remove_consensus(&self, relay_parent: &Hash) {
		self.inner.live_consensus.write().remove(relay_parent);
	}
}

// data needed for validating gossip.
pub(crate) struct MessageValidationData {
	/// The authorities at a block.
	pub(crate) authorities: Vec<SessionKey>,
}

impl MessageValidationData {
	fn check_statement(&self, relay_parent: &Hash, statement: &SignedStatement) -> bool {
		self.authorities.contains(&statement.sender) &&
			::polkadot_consensus::check_statement(
				&statement.statement,
				&statement.signature,
				statement.sender,
				relay_parent,
			)
	}
}

/// An unregistered message validator. Register this with `register_validator`.
pub struct MessageValidator<O: ?Sized> {
	live_consensus: RwLock<HashMap<Hash, MessageValidationData>>,
	oracle: O,
}

impl<O: KnownOracle + ?Sized> network_gossip::Validator<Hash> for MessageValidator<O> {
	fn validate(&self, mut data: &[u8]) -> GossipValidationResult<Hash> {
		match GossipMessage::decode(&mut data) {
			Some(GossipMessage { relay_parent, statement }) => {
				let live = self.live_consensus.read();
				let topic = || ::consensus::attestation_topic(relay_parent.clone());
				if let Some(validation) = live.get(&relay_parent) {
					if validation.check_statement(&relay_parent, &statement) {
						GossipValidationResult::Valid(topic())
					} else {
						GossipValidationResult::Invalid
					}
				} else {
					match self.oracle.is_known(&relay_parent) {
						None | Some(Known::Leaf) => GossipValidationResult::Future(topic()),
						Some(Known::Old) => GossipValidationResult::Expired,
						Some(Known::Bad) => GossipValidationResult::Invalid,
					}
				}
			}
			None => {
				debug!(target: "consensus", "Error decoding gossip message");
				GossipValidationResult::Invalid
			}
		}
	}
}
