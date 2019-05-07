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
use polkadot_primitives::parachain::{
	CandidateReceipt, HeadData, PoVBlock, BlockData, CollatorId, ValidatorId,
	ConsolidatedIngressRoots,
};
use substrate_primitives::crypto::UncheckedInto;
use codec::Encode;
use substrate_network::{
	PeerId, PeerInfo, ClientHandle, Context, config::Roles,
	message::{BlockRequest, generic::ConsensusMessage},
	specialization::NetworkSpecialization, generic_message::Message as GenericMessage
};

use futures::Future;

mod validation;

#[derive(Default)]
struct TestContext {
	disabled: Vec<PeerId>,
	disconnected: Vec<PeerId>,
	messages: Vec<(PeerId, Vec<u8>)>,
}

impl Context<Block> for TestContext {
	fn client(&self) -> &ClientHandle<Block> {
		unimplemented!()
	}

	fn report_peer(&mut self, peer: PeerId, reputation: i32) {
		match reputation {
			i if i < -100 => self.disabled.push(peer),
			i if i < 0 => self.disconnected.push(peer),
			_ => {}
		}
	}

	fn peer_info(&self, _peer: &PeerId) -> Option<PeerInfo<Block>> {
		unimplemented!()
	}

	fn send_block_request(&mut self, _who: PeerId, _request: BlockRequest<Block>) {
		unimplemented!()
	}

	fn send_consensus(&mut self, _who: PeerId, _consensus: ConsensusMessage) {
		unimplemented!()
	}

	fn send_chain_specific(&mut self, who: PeerId, message: Vec<u8>) {
		self.messages.push((who, message))
	}

	fn disconnect_peer(&mut self, _who: PeerId) { }
}

impl TestContext {
	fn has_message(&self, to: PeerId, message: Message) -> bool {
		let encoded = message.encode();
		self.messages.iter().any(|&(ref peer, ref data)|
			peer == &to && data == &encoded
		)
	}
}


fn make_pov(block_data: Vec<u8>) -> PoVBlock {
	PoVBlock {
		block_data: BlockData(block_data),
		ingress: polkadot_primitives::parachain::ConsolidatedIngress(Vec::new()),
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
	let canon_roots = ConsolidatedIngressRoots(Vec::new());
	let recv = protocol.fetch_pov_block(
		&mut TestContext::default(),
		&candidate_receipt,
		parent_hash,
		canon_roots,
	);

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
		assert!(ctx.has_message(peer_a.clone(), Message::RequestPovBlock(1, parent_hash, candidate_hash)));
	}

	knowledge.lock().note_statement(b_key.clone(), &GenericStatement::Valid(candidate_hash));

	// peer B connects and sends session key. request already assigned to A
	{
		let mut ctx = TestContext::default();
		protocol.on_connect(&mut ctx, peer_b.clone(), make_status(&status, Roles::AUTHORITY));
		on_message(&mut protocol, &mut ctx, peer_b.clone(), Message::SessionKey(b_key.clone()));
		assert!(!ctx.has_message(peer_b.clone(), Message::RequestPovBlock(2, parent_hash, candidate_hash)));

	}

	// peer A disconnects, triggering reassignment
	{
		let mut ctx = TestContext::default();
		protocol.on_disconnect(&mut ctx, peer_a.clone());
		assert!(!protocol.validators.contains_key(&a_key));
		assert!(ctx.has_message(peer_b.clone(), Message::RequestPovBlock(2, parent_hash, candidate_hash)));
	}

	// peer B comes back with block data.
	{
		let mut ctx = TestContext::default();
		let pov_block = make_pov(block_data.0);
		on_message(&mut protocol, &mut ctx, peer_b, Message::PovBlock(2, Some(pov_block.clone())));
		drop(protocol);
		assert_eq!(recv.wait().unwrap(), pov_block);
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
