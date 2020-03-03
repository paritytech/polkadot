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

//! Tests for the protocol.

use super::*;
use parking_lot::Mutex;

struct MockNetworkOps {
	recorded: Arc<Mutex<Recorded>>,
}

struct Recorded {
	peer_reputations: HashMap<PeerId, i32>,
	notifications: Vec<(PeerId, Message)>,
}

impl NetworkServiceOps for MockNetworkOps {
	fn report_peer(&self, peer: PeerId, value: sc_network::ReputationChange) {
		let mut recorded = self.recorded.lock();
		let total_rep = recorded.peer_reputations.entry(peer).or_insert(0);

		*total_rep = total_rep.saturating_add(value.value);
	}

	fn write_notification(
		&self,
		peer: PeerId,
		engine_id: ConsensusEngineId,
		notification: Vec<u8>,
	) {
		assert_eq!(engine_id, POLKADOT_ENGINE_ID);
		let message = Message::decode(&mut &notification[..]).expect("invalid notification");
		self.recorded.lock().notifications.push((peer, message));
	}
}

#[test]
fn router_inner_drop_sends_worker_message() {
	let parent = [1; 32].into();

	let (sender, mut receiver) = mpsc::channel(0);
	drop(RouterInner {
		relay_parent: parent,
		sender,
	});

	match receiver.try_next() {
		Ok(Some(ServiceToWorkerMsg::DropConsensusNetworking(x))) => assert_eq!(parent, x),
		_ => panic!("message not sent"),
	}
}
