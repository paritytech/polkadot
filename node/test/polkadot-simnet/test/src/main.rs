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
//! Attempts to upgrade the polkadot runtime, in a Simnet environment
use std::{error::Error, str::FromStr};

use polkadot_runtime::Event;
use polkadot_simnet::{dispatch_with_root, run};
use sc_client_api::{CallExecutor, ExecutorProvider};
use sp_blockchain::HeaderBackend;
use sp_core::crypto::AccountId32;
use sp_runtime::generic::BlockId;

fn main() -> Result<(), Box<dyn Error>> {
	run(|node| async {
		let old_runtime_version = node
			.client()
			.executor()
			.runtime_version(&BlockId::Hash(node.client().info().best_hash))?
			.spec_version;

		let wasm_binary = polkadot_runtime::WASM_BINARY
			.ok_or("Polkadot development wasm not available")?
			.to_vec();
		// upgrade runtime.
		dispatch_with_root(system::Call::set_code { code: wasm_binary }, &node).await?;

		// assert that the runtime has been updated by looking at events
		let events = node
			.events()
			.into_iter()
			.filter(|event| match event.event {
				Event::System(system::Event::CodeUpdated) => true,
				_ => false,
			})
			.collect::<Vec<_>>();

		// make sure event was emitted
		assert_eq!(
			events.len(),
			1,
			"system::Event::CodeUpdate not found in events: {:#?}",
			node.events()
		);
		let new_runtime_version = node
			.client()
			.executor()
			.runtime_version(&BlockId::Hash(node.client().info().best_hash))?
			.spec_version;
		// just confirming
		assert!(
			new_runtime_version > old_runtime_version,
			"Invariant, spec_version of new runtime: {} not greater than spec_version of old runtime: {}",
			new_runtime_version,
			old_runtime_version,
		);

		let (from, dest, balance) = (
			AccountId32::from_str("15j4dg5GzsL1bw2U2AWgeyAk6QTxq43V7ZPbXdAmbVLjvDCK")?,
			AccountId32::from_str("1rvXMZpAj9nKLQkPFCymyH7Fg3ZyKJhJbrc7UtHbTVhJm1A")?,
			10_000_000_000_000, // 10 dots
		);

		// post upgrade tests, a simple balance transfer
		node.submit_extrinsic(balances::Call::transfer { dest: dest.into(), value: balance }, from)
			.await?;
		node.seal_blocks(1).await;

		let events = node
			.events()
			.into_iter()
			.filter(|event| match event.event {
				Event::Balances(balances::Event::Transfer(_, _, _)) => true,
				_ => false,
			})
			.collect::<Vec<_>>();
		// make sure transfer went through
		assert_eq!(
			events.len(), 1,
			"balances::Call::transfer failed to execute, balances::Event::Transfer not found in events: {:#?}",
			node.events()
		);

		// we're done, drop node.
		drop(node);

		Ok(())
	})
}
