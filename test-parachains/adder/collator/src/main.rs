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

//! Collator for polkadot

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use adder::{HeadData as AdderHead, BlockData as AdderBody};
use substrate_primitives::{Pair, Blake2Hasher};
use parachain::codec::{Encode, Decode};
use primitives::{
	Hash, Block,
	parachain::{
		HeadData, BlockData, Id as ParaId, Message, OutgoingMessages, Status as ParachainStatus,
	},
};
use collator::{
	InvalidHead, ParachainContext, VersionInfo, Network, BuildParachainContext, TaskExecutor,
};
use parking_lot::Mutex;

const GENESIS: AdderHead = AdderHead {
	number: 0,
	parent_hash: [0; 32],
	post_state: [
		1, 27, 77, 3, 221, 140, 1, 241, 4, 145, 67, 207, 156, 76, 129, 126, 75,
		22, 127, 29, 27, 131, 229, 198, 240, 241, 13, 137, 186, 30, 123, 206
	],
};

const GENESIS_BODY: AdderBody = AdderBody {
	state: 0,
	add: 0,
};

#[derive(Clone)]
struct AdderContext {
	db: Arc<Mutex<HashMap<AdderHead, AdderBody>>>,
	/// We store it here to make sure that our interfaces require the correct bounds.
	_network: Option<Arc<dyn Network>>,
}

/// The parachain context.
impl ParachainContext for AdderContext {
	type ProduceCandidate = Result<(BlockData, HeadData, OutgoingMessages), InvalidHead>;

	fn produce_candidate<I: IntoIterator<Item=(ParaId, Message)>>(
		&mut self,
		_relay_parent: Hash,
		status: ParachainStatus,
		ingress: I,
	) -> Result<(BlockData, HeadData, OutgoingMessages), InvalidHead>
	{
		let adder_head = AdderHead::decode(&mut &status.head_data.0[..])
			.map_err(|_| InvalidHead)?;

		let mut db = self.db.lock();

		let last_body = if adder_head == GENESIS {
			GENESIS_BODY
		} else {
			db.get(&adder_head)
				.expect("All past bodies stored since this is the only collator")
				.clone()
		};

		let next_body = AdderBody {
			state: last_body.state.overflowing_add(last_body.add).0,
			add: adder_head.number % 100,
		};

		let from_messages = ::adder::process_messages(
			ingress.into_iter().map(|(_, msg)| msg.0)
		);

		let next_head = ::adder::execute(adder_head.hash(), adder_head, &next_body, from_messages)
			.expect("good execution params; qed");

		let encoded_head = HeadData(next_head.encode());
		let encoded_body = BlockData(next_body.encode());

		println!("Created collation for #{}, post-state={}",
			next_head.number, next_body.state.overflowing_add(next_body.add).0);

		db.insert(next_head.clone(), next_body);
		Ok((encoded_body, encoded_head, OutgoingMessages { outgoing_messages: Vec::new() }))
	}
}

impl BuildParachainContext for AdderContext {
	type ParachainContext = Self;

	fn build<B, E>(
		self,
		_: Arc<collator::PolkadotClient<B, E>>,
		_: TaskExecutor,
		network: Arc<dyn Network>,
	) -> Result<Self::ParachainContext, ()>
		where
			B: client_api::backend::Backend<Block, Blake2Hasher> + 'static,
			E: client::CallExecutor<Block, Blake2Hasher> + Clone + Send + Sync + 'static
	{
		Ok(Self { _network: Some(network), ..self })
	}
}

fn main() {
	let key = Arc::new(Pair::from_seed(&[1; 32]));
	let id: ParaId = 100.into();

	println!("Starting adder collator with genesis: ");

	{
		let encoded = GENESIS.encode();
		println!("Dec: {:?}", encoded);
		print!("Hex: 0x");
		for byte in encoded {
			print!("{:02x}", byte);
		}

		println!();
	}

	// can't use signal directly here because CtrlC takes only `Fn`.
	let (exit_send, exit) = exit_future::signal();

	let exit_send_cell = RefCell::new(Some(exit_send));
	ctrlc::set_handler(move || {
		if let Some(exit_send) = exit_send_cell.try_borrow_mut().expect("signal handler not reentrant; qed").take() {
			exit_send.fire();
		}
	}).expect("Error setting up ctrl-c handler");

	let context = AdderContext {
		db: Arc::new(Mutex::new(HashMap::new())),
		_network: None,
	};

	let res = ::collator::run_collator(
		context,
		id,
		exit,
		key,
		VersionInfo {
			name: "<unknown>",
			version: "<unknown>",
			commit: "<unknown>",
			executable_name: "adder-collator",
			description: "collator for adder parachain",
			author: "parity technologies",
			support_url: "https://github.com/paritytech/polkadot/issues/new",
		}
	);

	if let Err(e) = res {
		println!("{}", e);
	}
}
