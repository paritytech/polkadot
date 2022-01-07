// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use node_primitives::Block;
use remote_externalities::rpc_api::get_finalized_head;
use std::{
	process::{Child, ExitStatus},
	thread,
	time::Duration,
};
use tokio::time::timeout;

static LOCALHOST_WS: &str = "ws://127.0.0.1:9944/";

/// Wait for the given `child` the given ammount of `secs`.
///
/// Returns the `Some(exit status)` or `None` if the process did not finish in the given time.
pub fn wait_for(child: &mut Child, secs: usize) -> Option<ExitStatus> {
	for _ in 0..secs {
		match child.try_wait().unwrap() {
			Some(status) => return Some(status),
			None => thread::sleep(Duration::from_secs(1)),
		}
	}
	eprintln!("Took to long to exit. Killing...");
	let _ = child.kill();
	child.wait().unwrap();

	None
}

/// Wait for at least `n` blocks to be finalized within the specified time.
pub async fn wait_n_finalized_blocks(
	n: usize,
	timeout_duration: Duration,
) -> Result<(), tokio::time::error::Elapsed> {
	timeout(timeout_duration, wait_n_finalized_blocks_from(n, LOCALHOST_WS)).await
}

/// Wait for at least `n` blocks to be finalized from a specified node.
async fn wait_n_finalized_blocks_from(n: usize, url: &str) {
	let mut built_blocks = std::collections::HashSet::new();
	let mut interval = tokio::time::interval(Duration::from_secs(6));

	loop {
		if let Ok(block) = get_finalized_head::<Block, _>(url.to_string()).await {
			built_blocks.insert(block);
			if built_blocks.len() > n {
				break
			}
		};
		interval.tick().await;
	}
}
