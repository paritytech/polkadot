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

use polkadot_core_primitives::{Block, Hash, Header};
use std::{
	io::{BufRead, BufReader, Read},
	process::{Child, ExitStatus},
	thread,
	time::Duration,
};
use substrate_rpc_client::{ws_client, ChainApi};
use tokio::time::timeout;

/// Wait for the given `child` the given amount of `secs`.
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
	url: &str,
) -> Result<(), tokio::time::error::Elapsed> {
	timeout(timeout_duration, wait_n_finalized_blocks_from(n, url)).await
}

/// Wait for at least `n` blocks to be finalized from a specified node.
async fn wait_n_finalized_blocks_from(n: usize, url: &str) {
	let mut built_blocks = std::collections::HashSet::new();
	let mut interval = tokio::time::interval(Duration::from_secs(6));

	loop {
		let rpc = match ws_client(url).await {
			Ok(rpc_service) => rpc_service,
			Err(_) => continue,
		};

		if let Ok(block) = ChainApi::<(), Hash, Header, Block>::finalized_head(&rpc).await {
			built_blocks.insert(block);
			if built_blocks.len() > n {
				break
			}
		};
		interval.tick().await;
	}
}

/// Read the WS address from the output.
///
/// This is hack to get the actual binded sockaddr because
/// polkadot assigns a random port if the specified port was already binded.
///
/// You must call `Command::new("cmd").stdout(process::Stdio::piped()).stderr(process::Stdio::piped())`
/// for this to work.
pub fn find_ws_url_from_output(read: impl Read + Send) -> (String, String) {
	let mut data = String::new();

	let ws_url = BufReader::new(read)
		.lines()
		.find_map(|line| {
			let line = line.expect("failed to obtain next line from stdout for port discovery");

			data.push_str(&line);

			// does the line contain our port (we expect this specific output from substrate).
			let sock_addr = match line.split_once("Running JSON-RPC WS server: addr=") {
				None => return None,
				Some((_, after)) => after.split_once(',').unwrap().0,
			};

			Some(format!("ws://{}", sock_addr))
		})
		.unwrap_or_else(|| {
			panic!("Could not find WebSocket address in process output:\n{}", &data)
		});
	(ws_url, data)
}
