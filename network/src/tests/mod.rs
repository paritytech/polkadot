// Copyright 2018 Parity Technologies (UK) Ltd.
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

//! Tests for polkadot and validation network.

use super::{PolkadotProtocol, Status, Message, FullStatus};
use validation::SessionParams;

use polkadot_validation::GenericStatement;
use polkadot_primitives::{Block, Hash, SessionKey};
use polkadot_primitives::parachain::{CandidateReceipt, HeadData, BlockData, CollatorId, ValidatorId};
use sr_primitives::Justification;
use sr_primitives::generic::BlockId;
use sr_primitives::traits::NumberFor;
use substrate_primitives::H256;
use substrate_primitives::crypto::UncheckedInto;
use codec::Encode;
use substrate_consensus::import_queue::{SharedBlockImport, SharedJustificationImport, Verifier};
use substrate_consensus::{Error as ConsensusError, ErrorKind as ConsensusErrorKind, JustificationImport};
use substrate_network::{
	Severity, PeerId, PeerInfo, ClientHandle, Context, config::Roles,
	message::Message as SubstrateMessage, specialization::NetworkSpecialization,
	generic_message::Message as GenericMessage
};
use substrate_network::test::{
	Block, DummySpecialization, Hash, TestNet, TestNetFactory,
	PassThroughVerifier, Peer, PeersClient, ProtocolConfig, SpecializationFactory
};

use futures::Future;
use std::sync::Arc;

mod validation;

impl SpecializationFactory for PolkadotProtocol {
	fn create() -> PolkadotProtocol {
		PolkadotProtocol::new(None)
	}
}

pub struct ForceFinalized(Arc<PeersClient>);

impl JustificationImport<Block> for ForceFinalized {
	type Error = ConsensusError;

	fn import_justification(
		&self,
		hash: H256,
		_number: NumberFor<Block>,
		justification: Justification,
	) -> Result<(), Self::Error> {
		self.0.finalize_block(BlockId::Hash(hash), Some(justification), true)
			.map_err(|_| ConsensusErrorKind::InvalidJustification.into())
	}
}

struct PolkadotTestNet(TestNet);

impl TestNetFactory for PolkadotTestNet {
	type Specialization = PolkadotProtocol;
	type Verifier = PassThroughVerifier;
	type PeerData = ();

	fn from_config(config: &ProtocolConfig) -> Self {
		PolkadotTestNet(TestNet::from_config(config))
	}

	fn make_verifier(&self, client: Arc<PeersClient>, config: &ProtocolConfig)
		-> Arc<Self::Verifier>
	{
		self.0.make_verifier(client, config)
	}

	fn peer(&self, i: usize) -> &Peer<Self::PeerData, Self::Specialization> {
		self.0.peer(i)
	}

	fn peers(&self) -> &Vec<Arc<Peer<Self::PeerData, Self::Specialization>>> {
		self.0.peers()
	}

	fn mut_peers<F: FnOnce(&mut Vec<Arc<Peer<Self::PeerData, Self::Specialization>>>)>(&mut self, closure: F ) {
		self.0.mut_peers(closure)
	}

	fn started(&self) -> bool {
		self.0.started()
	}

	fn set_started(&mut self, new: bool) {
		self.0.set_started(new)
	}

	fn make_block_import(&self, client: Arc<PeersClient>)
		-> (SharedBlockImport<Block>, Option<SharedJustificationImport<Block>>, Self::PeerData)
	{
		(client.clone(), Some(Arc::new(ForceFinalized(client))), Default::default())
	}
}


#[derive(Default)]
struct TestContext {
	disabled: Vec<PeerId>,
	disconnected: Vec<PeerId>,
	messages: Vec<(PeerId, SubstrateMessage<Block>)>,
}

impl Context<Block> for TestContext {
	fn client(&self) -> &ClientHandle<Block> {
		unimplemented!()
	}

	fn report_peer(&mut self, peer: PeerId, reason: Severity) {
		match reason {
			Severity::Bad(_) => self.disabled.push(peer),
			_ => self.disconnected.push(peer),
		}
	}

	fn peer_info(&self, _peer: &PeerId) -> Option<PeerInfo<Block>> {
		unimplemented!()
	}

	fn send_message(&mut self, who: PeerId, data: SubstrateMessage<Block>) {
		self.messages.push((who, data))
	}
}

impl TestContext {
	fn has_message(&self, to: PeerId, message: Message) -> bool {
		use substrate_network::generic_message::Message as GenericMessage;

		let encoded = message.encode();
		self.messages.iter().any(|&(ref peer, ref msg)| match msg {
			GenericMessage::ChainSpecific(ref data) => peer == &to && data == &encoded,
			_ => false,
		})
	}
}

fn make_status(status: &Status, roles: Roles) -> FullStatus {
	FullStatus {
		version: 1,
		min_supported_version: 1,
		roles,
		best_number: 0,
		best_hash: Default::default(),
		genesis_hash: Default::default(),
		chain_status: status.encode(),
	}
}

fn make_validation_session(parent_hash: Hash, local_key: SessionKey) -> SessionParams {
	SessionParams {
		local_session_key: Some(local_key),
		parent_hash,
		authorities: Vec::new(),
	}
}

fn on_message(protocol: &mut PolkadotProtocol, ctx: &mut TestContext, from: PeerId, message: Message) {
	let encoded = message.encode();
	protocol.on_message(ctx, from, &mut Some(GenericMessage::ChainSpecific(encoded)));
}

#[test]
fn sends_session_key() {
	let mut protocol = PolkadotProtocol::new(None);

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let parent_hash = [0; 32].into();
	let local_key: ValidatorId = [1; 32].unchecked_into();

	let validator_status = Status { collating_for: None };
	let collator_status = Status { collating_for: Some(([2; 32].unchecked_into(), 5.into())) };

	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, peer_a.clone(), make_status(&validator_status, Roles::AUTHORITY));
		assert!(ctx.messages.is_empty());
	}

	{
		let mut ctx = TestContext::default();
		let params = make_validation_session(parent_hash, local_key.clone());
		protocol.new_validation_session(&mut ctx, params);
		assert!(ctx.has_message(peer_a, Message::SessionKey(local_key.clone())));
	}

	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, peer_b.clone(), make_status(&collator_status, Roles::NONE));
		assert!(ctx.has_message(peer_b.clone(), Message::SessionKey(local_key)));
	}
}

#[test]
fn fetches_from_those_with_knowledge() {
	let mut protocol = PolkadotProtocol::new(None);

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let parent_hash = [0; 32].into();
	let local_key: ValidatorId = [1; 32].unchecked_into();

	let block_data = BlockData(vec![1, 2, 3, 4]);
	let block_data_hash = block_data.hash();
	let candidate_receipt = CandidateReceipt {
		parachain_index: 5.into(),
		collator: [255; 32].unchecked_into(),
		head_data: HeadData(vec![9, 9, 9]),
		signature: Default::default(),
		balance_uploads: Vec::new(),
		egress_queue_roots: Vec::new(),
		fees: 1_000_000,
		block_data_hash,
	};

	let candidate_hash = candidate_receipt.hash();
	let a_key: ValidatorId = [3; 32].unchecked_into();
	let b_key: ValidatorId = [4; 32].unchecked_into();

	let status = Status { collating_for: None };

	let params = make_validation_session(parent_hash, local_key.clone());
	let session = protocol.new_validation_session(&mut TestContext::default(), params);
	let knowledge = session.knowledge();

	knowledge.lock().note_statement(a_key.clone(), &GenericStatement::Valid(candidate_hash));
	let recv = protocol.fetch_block_data(&mut TestContext::default(), &candidate_receipt, parent_hash);

	// connect peer A
	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, peer_a.clone(), make_status(&status, Roles::AUTHORITY));
		assert!(ctx.has_message(peer_a.clone(), Message::SessionKey(local_key)));
	}

	// peer A gives session key and gets asked for data.
	{
		let mut ctx = TestContext::default();
		on_message(&mut protocol, &mut ctx, peer_a.clone(), Message::SessionKey(a_key.clone()));
		assert!(protocol.validators.contains_key(&a_key));
		assert!(ctx.has_message(peer_a.clone(), Message::RequestBlockData(1, parent_hash, candidate_hash)));
	}

	knowledge.lock().note_statement(b_key.clone(), &GenericStatement::Valid(candidate_hash));

	// peer B connects and sends session key. request already assigned to A
	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, peer_b.clone(), make_status(&status, Roles::AUTHORITY));
		on_message(&mut protocol, &mut ctx, peer_b.clone(), Message::SessionKey(b_key.clone()));
		assert!(!ctx.has_message(peer_b.clone(), Message::RequestBlockData(2, parent_hash, candidate_hash)));

	}

	// peer A disconnects, triggering reassignment
	{
		let mut ctx = TestContext::default();
		protocol.on_disconnect(&mut ctx, peer_a.clone());
		assert!(!protocol.validators.contains_key(&a_key));
		assert!(ctx.has_message(peer_b.clone(), Message::RequestBlockData(2, parent_hash, candidate_hash)));
	}

	// peer B comes back with block data.
	{
		let mut ctx = TestContext::default();
		on_message(&mut protocol, &mut ctx, peer_b, Message::BlockData(2, Some(block_data.clone())));
		drop(protocol);
		assert_eq!(recv.wait().unwrap(), block_data);
	}
}

#[test]
fn fetches_available_block_data() {
	let mut protocol = PolkadotProtocol::new(None);

	let peer_a = PeerId::random();
	let parent_hash = [0; 32].into();

	let block_data = BlockData(vec![1, 2, 3, 4]);
	let block_data_hash = block_data.hash();
	let para_id = 5.into();
	let candidate_receipt = CandidateReceipt {
		parachain_index: para_id,
		collator: [255; 32].unchecked_into(),
		head_data: HeadData(vec![9, 9, 9]),
		signature: Default::default(),
		balance_uploads: Vec::new(),
		egress_queue_roots: Vec::new(),
		fees: 1_000_000,
		block_data_hash,
	};

	let candidate_hash = candidate_receipt.hash();
	let av_store = ::av_store::Store::new_in_memory();

	let status = Status { collating_for: None };

	protocol.register_availability_store(av_store.clone());

	av_store.make_available(::av_store::Data {
		relay_parent: parent_hash,
		parachain_id: para_id,
		candidate_hash,
		block_data: block_data.clone(),
		extrinsic: None,
	}).unwrap();

	// connect peer A
	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, peer_a.clone(), make_status(&status, Roles::FULL));
	}

	// peer A asks for historic block data and gets response
	{
		let mut ctx = TestContext::default();
		on_message(&mut protocol, &mut ctx, peer_a.clone(), Message::RequestBlockData(1, parent_hash, candidate_hash));
		assert!(ctx.has_message(peer_a, Message::BlockData(1, Some(block_data))));
	}
}

#[test]
fn remove_bad_collator() {
	let mut protocol = PolkadotProtocol::new(None);

	let who = PeerId::random();
	let collator_id: CollatorId = [2; 32].unchecked_into();

	let status = Status { collating_for: Some((collator_id.clone(), 5.into())) };

	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, who.clone(), make_status(&status, Roles::NONE));
	}

	{
		let mut ctx = TestContext::default();
		protocol.disconnect_bad_collator(&mut ctx, collator_id);
		assert!(ctx.disabled.contains(&who));
	}
}

#[test]
fn many_session_keys() {
	let mut protocol = PolkadotProtocol::new(None);

	let parent_a = [1; 32].into();
	let parent_b = [2; 32].into();

	let local_key_a: ValidatorId = [3; 32].unchecked_into();
	let local_key_b: ValidatorId = [4; 32].unchecked_into();

	let params_a = make_validation_session(parent_a, local_key_a.clone());
	let params_b = make_validation_session(parent_b, local_key_b.clone());

	protocol.new_validation_session(&mut TestContext::default(), params_a);
	protocol.new_validation_session(&mut TestContext::default(), params_b);

	assert_eq!(protocol.live_validation_sessions.recent_keys(), &[local_key_a.clone(), local_key_b.clone()]);

	let peer_a = PeerId::random();

	// when connecting a peer, we should get both those keys.
	{
		let mut ctx = TestContext::default();

		let status = Status { collating_for: None };
		protocol.on_connect(&mut ctx, peer_a.clone(), make_status(&status, Roles::AUTHORITY));

		assert!(ctx.has_message(peer_a.clone(), Message::SessionKey(local_key_a.clone())));
		assert!(ctx.has_message(peer_a, Message::SessionKey(local_key_b.clone())));
	}

	let peer_b = PeerId::random();

	assert!(protocol.remove_validation_session(parent_a));

	{
		let mut ctx = TestContext::default();

		let status = Status { collating_for: None };
		protocol.on_connect(&mut ctx, peer_b.clone(), make_status(&status, Roles::AUTHORITY));

		assert!(!ctx.has_message(peer_b.clone(), Message::SessionKey(local_key_a.clone())));
		assert!(ctx.has_message(peer_b, Message::SessionKey(local_key_b.clone())));
	}
}
