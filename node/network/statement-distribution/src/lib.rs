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

use futures::prelude::*;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, BTreeSet};

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

struct PeerData {
	role: ObservedRole,
	view: View,
	view_knowledge: HashMap<Hash, PeerRelayParentKnowledge>,
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

impl PartialOrd for StoredStatement {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for StoredStatement {
	fn cmp(&self, other: &Self) -> Ordering {
		let to_idx = |x: &CompactStatement| match *x {
			CompactStatement::Candidate(_) => 0u8,
			CompactStatement::Valid(_) => 1,
			CompactStatement::Invalid(_) => 2,
		};

		match (&self.compact, &other.compact) {
			(&CompactStatement::Candidate(ref h), &CompactStatement::Candidate(ref h2)) |
			(&CompactStatement::Valid(ref h), &CompactStatement::Valid(ref h2)) |
			(&CompactStatement::Invalid(ref h), &CompactStatement::Invalid(ref h2)) => {
				h.cmp(h2).then(
					self.statement.validator_index().cmp(&other.statement.validator_index())
				)
			},

			(ref a, ref b) => to_idx(a).cmp(&to_idx(b)),
		}
	}
}

struct ActiveHeadData {
	// All candidates we are aware of for this head, keyed by hash.
	candidates: HashSet<Hash>,
	// Stored statements for circulation to peers.
	statements: BTreeSet<StoredStatement>,
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

			}
			FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {

			}
			FromOverseer::Signal(OverseerSignal::Conclude) => break,
			FromOverseer::Communication { msg } => match msg {
				StatementDistributionMessage::Share(relay_parent, statement) => {
					// place into `active_heads` and circulate to all peers with
					// the head in their view.
				}
				StatementDistributionMessage::NetworkBridgeUpdate(event) => match event {
					_ => unimplemented!(),
				}
			}
		}
	}
	Ok(())
}
