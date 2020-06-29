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
use polkadot_primitives::parachain::{
	CompactStatement, ValidatorIndex, ValidatorId, SigningContext,
};
use parity_scale_codec::{Encode, Decode};

use futures::prelude::*;

use std::collections::{HashMap, HashSet};

const PROTOCOL_V1: ProtocolId = *b"sdn1";

const COST_UNEXPECTED_STATEMENT: Rep = Rep::new(-100, "Unexpected Statement");
const COST_INVALID_SIGNATURE: Rep = Rep::new(-500, "Invalid Statement Signature");
const COST_INVALID_MESSAGE: Rep = Rep::new(-500, "Invalid message");

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
	// How many candidates this peer is aware of for each given validator index.
	seconded_counts: HashMap<ValidatorIndex, usize>,
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
				// Each peer is allowed to be aware of two seconded statements per validator per
				// relay-parent hash.
				let count = self.seconded_counts.entry(fingerprint.1).or_insert(0);
				if *count >= 2 {
					return false;
				}
				*count += 1;

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
	/// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<Hash>,
	/// Stored seconded statements for circulation to peers.
	seconded_statements: HashSet<StoredStatement>,
	/// Stored other statements for circulation to peers.
	other_statements: HashSet<StoredStatement>,
	/// The validators at this head.
	validators: Vec<ValidatorId>,
	/// The session index this head is at.
	session_index: sp_staking::SessionIndex,
}

impl ActiveHeadData {
	fn new(validators: Vec<ValidatorId>, session_index: sp_staking::SessionIndex) -> Self {
		ActiveHeadData {
			candidates: Default::default(),
			seconded_statements: Default::default(),
			other_statements: Default::default(),
			validators,
			session_index,
		}
	}

	/// Note the given statement.
	///
	/// If it was not already known and can be accepted,  returns `Some`,
	/// with a handle to the statement.
	///
	/// `Seconded` statements are always accepted, and are assumed to have passed flood-mitigation
	/// measures before reaching this point.
	///
	/// Other statements that reference a candidate we are not aware of cannot be accepted.
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
			CompactStatement::Valid(h) | CompactStatement::Invalid(h) => {
				if !self.candidates.contains(&h) {
					return None;
				}

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

/// Check a statement signature under this parent hash.
fn check_statement_signature(
	head: &ActiveHeadData,
	relay_parent: Hash,
	statement: &SignedFullStatement,
) -> Result<(), ()> {
	let signing_context = SigningContext {
		session_index: head.session_index,
		parent_hash: relay_parent,
	};

	match head.validators.get(statement.validator_index() as usize) {
		None => return Err(()),
		Some(v) => statement.check_signature(&signing_context, v),
	}
}

#[derive(Encode, Decode)]
enum WireMessage {
	/// relay-parent, full statement.
	Statement(Hash, SignedFullStatement),
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
		send_stored_to_peers(peers, ctx, relay_parent, stored).await?;
	}
	Ok(())
}

async fn send_stored_to_peers(
	peers: &mut HashMap<PeerId, PeerData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	relay_parent: Hash,
	stored: &StoredStatement,
) -> SubsystemResult<()> {
	let fingerprint = stored.fingerprint();
	let peers_to_send: Vec<_> = peers.iter_mut()
		.filter_map(|(p, data)| if data.accept(&relay_parent, &fingerprint) {
			Some(p.clone())
		} else {
			None
		})
		.collect();

	if peers_to_send.is_empty() { return Ok(()) }

	let payload = WireMessage::Statement(relay_parent, stored.statement.clone()).encode();
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
		peers_to_send,
		PROTOCOL_V1,
		payload,
	))).await?;

	Ok(())
}

async fn report_peer(
	ctx: &mut impl SubsystemContext,
	peer: PeerId,
	rep: Rep,
) -> SubsystemResult<()> {
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep)
	)).await
}

// Handle an incoming wire message. Returns a reference to a newly-stored statement
// if we were not already aware of it, along with the corresponding relay-parent.
//
// This function checks the signature and ensures the statement is compatible with our
// view.
async fn handle_incoming_message<'a>(
	peer: PeerId,
	peer_data: &mut PeerData,
	our_view: &View,
	active_heads: &'a mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	message: Vec<u8>,
) -> SubsystemResult<Option<(Hash, &'a StoredStatement)>> {
	let (relay_parent, statement) = match WireMessage::decode(&mut &message[..]) {
		Err(_) => return report_peer(ctx, peer, COST_INVALID_MESSAGE).await.map(|_| None),
		Ok(WireMessage::Statement(r, s)) => (r, s),
	};

	if !our_view.contains(relay_parent) {
		return report_peer(ctx, peer, COST_UNEXPECTED_STATEMENT).await.map(|_| None);
	}

	let active_head = match active_heads.get_mut(relay_parent) {
		Some(h) => h,
		None => {
			// This should never be out-of-sync with our view if the view updates
			// correspond to actual `StartWork` messages. So we just log and ignore.
			log::warn!("Our view out-of-sync with active heads. Head {} not found", relay_parent);
			return Ok(None);
		}
	};

	// check the signature on the statement.
	if let Err(()) = check_statement_signature(&active_head, relay_parent, &statement) {
		return report_peer(ctx, peer, COST_INVALID_SIGNATURE).await.map(|_| None);
	}

	// Ensure the statement is stored in the peer data.
	//
	// Note that if the peer is sending us something that is not within their view,
	// it will not be kept within their log.
	let fingerprint = (statement.payload().to_compact(), statement.validator_index());
	if !peer_data.accept(&relay_parent, fingerprint) {
		// If the peer was already aware of this statement or it was not within their own view,
		// then we note the peer as being costly.
		//
		// This can race if we send the message to the peer at exactly the same time,
		// but the report cost is relatively low, and the expected amounts of reports due to
		// that race is also low.
		//
		// Regardless, this serves as a deterrent for peers to avoid spamming us with the same
		// message over and over again, as we will disconnect from such peers.
		report_peer(ctx, peer, COST_UNEXPECTED_STATEMENT).await?;
	}

	// Note: `peer_data.accept` already ensures that the statement is not an unbounded equivocation
	// or unpinned to a seconded candidate. So it is safe to place it into the storage.
	Ok(active_head.note_statement(statement).map(|s| (relay_parent, s)))
}

async fn handle_network_update(
	peers: &mut HashMap<PeerId, PeerData>,
	active_heads: &mut HashMap<Hash, ActiveHeadData>,
	ctx: &mut impl SubsystemContext<Message = StatementDistributionMessage>,
	our_view: &mut View,
	update: NetworkBridgeEvent,
) -> SubsystemResult<()> {
	match update {
		NetworkBridgeEvent::PeerConnected(peer, role) => {
			peers.insert(peer, PeerData {
				role,
				view: Default::default(),
				view_knowledge: Default::default(),
			});

			Ok(())
		}
		NetworkBridgeEvent::PeerDisconnected(peer) => {
			peers.remove(&peer);
			Ok(())
		}
		NetworkBridgeEvent::PeerMessage(peer, message) => {
			match peers.get_mut(&peer) {
				Some(data) => handle_incoming_message(
					peer,
					data,
					&*our_view,
					active_heads,
					ctx,
					message,
				).await?,
				None => Ok(()),
			}

		}
		NetworkBridgeEvent::PeerViewChange(peer, view) => {
			// 1. Update the view.
			// 2. Send this peer all messages that we have for new active heads in the view.
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			// 1. Update our view.
			// 2. Clean up everything that is not in the new view.
		}
	}

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
				active_heads.entry(relay_parent)
					.or_insert(ActiveHeadData::new(unimplemented!(), unimplemented!()));
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
					&mut our_view,
					event,
				).await?,
			}
		}
	}
	Ok(())
}
