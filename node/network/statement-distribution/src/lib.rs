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

//! The Statement Distribution Subsystem.
//!
//! This is responsible for distributing signed statements about candidate
//! validity amongst validators.

use polkadot_subsystem::{
	Subsystem, SubsystemResult, SubsystemError, SubsystemContext, SpawnedSubsystem,
	FromOverseer, OverseerSignal,
};
use polkadot_subsystem::messages::{
	AllMessages, NetworkBridgeMessage, NetworkBridgeEvent, StatementDistributionMessage,
	PeerId, ObservedRole, ReputationChange as Rep,
};
use node_primitives::{ProtocolId, View, SignedFullStatement};
use polkadot_primitives::Hash;
use polkadot_primitives::parachain::{CompactStatement, ValidatorIndex};
use parity_scale_codec::{Encode, Decode};

use futures::prelude::*;

use std::collections::{HashMap, HashSet};

const PROTOCOL_V1: ProtocolId = *b"sdn1";

const COST_UNEXPECTED_STATEMENT: Rep = Rep::new(-100, "Unexpected Statement");
const COST_INVALID_SIGNATURE: Rep = Rep::new(-500, "Invalid Statement Signature");

const BENEFIT_VALID_STATEMENT: Rep = Rep::new(25, "Peer provided a valid statement");

/// The statement distribution subsystem.
pub struct StatementDistribution;

impl<C> Subsystem<C> for StatementDistribution
	where C: SubsystemContext<Message=StatementDistributionMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run`.
		SpawnedSubsystem(run(ctx).map(|_| ()).boxed())
	}
}

fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
	AllMessages::StatementDistribution(StatementDistributionMessage::NetworkBridgeUpdate(n))
}

// knowledge that a peer has about goings-on in a relay parent.
struct PeerRelayParentKnowledge {
	// candidates that a peer is aware of. This indicates that we can send
	// statements pertaining to that candidate.
	known_candidates: HashSet<Hash>,
	// fingerprints of all statements a peer should be aware of: those that
	// were sent to us by the peer or sent to the peer by us.
	known_statements: HashSet<(CompactStatement, ValidatorIndex)>,
}

impl PeerRelayParentKnowledge {
	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint.
	///
	/// This returns `false` if the peer cannot accept this statement, without altering internal
	/// state.
	///
	/// If the peer can accept the statement, this returns `true` and updates the internal state.
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	fn accept(&mut self, fingerprint: &(CompactStatement, ValidatorIndex)) -> bool {
		if self.known_statements.contains(fingerprint) {
			return false;
		}

		match fingerprint.0 {
			CompactStatement::Candidate(ref h) => {
				self.known_candidates.insert(h.clone());
			},
			CompactStatement::Valid(ref h) | CompactStatement::Invalid(ref h) => {
				// The peer can only accept Valid and Invalid statements for which it is aware
				// of the corresponding candidate.
				if self.known_candidates.contains(h) {
					return false;
				}
			}
		}

		self.known_statements.insert(fingerprint.clone());
		true
	}
}

struct PeerData {
	role: ObservedRole,
	view: View,
	view_knowledge: HashMap<Hash, PeerRelayParentKnowledge>,
}

impl PeerData {
	/// Attempt to update our view of the peer's knowledge with this statement's fingerprint.
	///
	/// This returns `false` if the peer cannot accept this statement, without altering internal
	/// state.
	///
	/// If the peer can accept the statement, this returns `true` and updates the internal state.
	/// Once the knowledge has incorporated a statement, it cannot be incorporated again.
	fn accept(
		&mut self,
		relay_parent: &Hash,
		fingerprint: &(CompactStatement, ValidatorIndex),
	) -> bool {
		self.view_knowledge.get_mut(relay_parent).map_or(false, |k| k.accept(fingerprint))
	}
}

// A statement stored while a relay chain head is active.
//
// These are orderable first by (Seconded, Valid, Invalid), then by the underlying hash,
// and lastly by the signing validator's index.
#[derive(PartialEq, Eq)]
struct StoredStatement {
	compact: CompactStatement,
	statement: SignedFullStatement,
}

impl StoredStatement {
	fn fingerprint(&self) -> (CompactStatement, ValidatorIndex) {
		(self.compact.clone(), self.statement.validator_index())
	}
}

impl std::borrow::Borrow<CompactStatement> for StoredStatement {
	fn borrow(&self) -> &CompactStatement {
		&self.compact
	}
}

impl std::hash::Hash for StoredStatement {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.fingerprint().hash(state)
	}
}

struct ActiveHeadData {
	// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<Hash>,
	// Stored seconded statements for circulation to peers.
	seconded_statements: HashSet<StoredStatement>,
	// Stored other statements for circulation to peers.
	other_statements: HashSet<StoredStatement>,
}

impl ActiveHeadData {
	/// Note the given statement. If it was not already known, returns `Some`, with a handle to the
	/// statement.
	fn note_statement(&mut self, statement: SignedFullStatement) -> Option<&StoredStatement> {
		let compact = statement.payload().to_compact();
		let stored = StoredStatement {
			compact: compact.clone(),
			statement,
		};

		match compact {
			CompactStatement::Candidate(h) => {
				self.candidates.insert(h);
				if self.seconded_statements.insert(stored) {
					// This will always return `Some` because it was just inserted.
					self.seconded_statements.get(&compact)
				} else {
					None
				}
			}
			CompactStatement::Valid(_) | CompactStatement::Invalid(_) => {
				if self.other_statements.insert(stored) {
					// This will always return `Some` because it was just inserted.
					self.other_statements.get(&compact)
				} else {
					None
				}
			}
		}
	}

	/// Get an iterator over all statements for the active head. Seconded statements come first.
	fn statements(&self) -> impl Iterator<Item = &'_ StoredStatement> + '_ {
		self.seconded_statements.iter().chain(self.other_statements.iter())
	}
}

async fn share_message(
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	relay_parent: Hash,
	statement: SignedFullStatement,
) -> SubsystemResult<()> {
	if let Some(stored)
		= active_heads.get_mut(&relay_parent).and_then(|d| d.note_statement(statement))
	{
		let fingerprint = stored.fingerprint();
		let peers_to_send: Vec<_> = peers.iter_mut()
			.filter_map(|(p, data)| if data.accept(&relay_parent, &fingerprint) {
				Some(p.clone())
			} else {
				None
			})
			.collect();

		if peers_to_send.is_empty() { return Ok(()) }

		let payload = stored.statement.encode();
		ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
			peers_to_send,
			PROTOCOL_V1,
			payload,
		))).await?;
	}
	Ok(())
}

async fn handle_network_update(
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	update: NetworkBridgeEvent,
) -> SubsystemResult<()> {
	Ok(())
}

async fn run(
	mut ctx: impl SubsystemContext<Message = StatementDistributionMessage>,
) -> SubsystemResult<()> {
	// startup: register the network protocol with the bridge.
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::RegisterEventProducer(
		PROTOCOL_V1,
		network_update_message,
	))).await?;

	let mut peers: HashMap<PeerId, PeerData> = HashMap::new();
	let mut our_view = View::default();
	let mut active_heads: HashMap<Hash, ActiveHeadData> = HashMap::new();

	loop {
		let message = ctx.recv().await?;
		match message {
			FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)) => {
				active_heads.entry(relay_parent).or_insert(ActiveHeadData {
					candidates: HashSet::new(),
					seconded_statements: HashSet::new(),
					other_statements: HashSet::new(),
				});
			}
			FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {
				// do nothing - we will handle this when our view changes.
			}
			FromOverseer::Signal(OverseerSignal::Conclude) => break,
			FromOverseer::Communication { msg } => match msg {
				StatementDistributionMessage::Share(relay_parent, statement) => share_message(
					&mut peers,
					&mut active_heads,
					&mut ctx,
					relay_parent,
					statement,
				).await?,
				StatementDistributionMessage::NetworkBridgeUpdate(event) => handle_network_update(
					&mut peers,
					&mut active_heads,
					&mut ctx,
					event,
				).await?,
			}
		}
	}
	Ok(())
}
