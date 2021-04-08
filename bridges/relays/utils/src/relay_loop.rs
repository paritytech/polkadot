// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use crate::{FailedClient, MaybeConnectionError};

use async_trait::async_trait;
use std::{fmt::Debug, future::Future, time::Duration};

/// Default pause between reconnect attempts.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(10);

/// Basic blockchain client from relay perspective.
#[async_trait]
pub trait Client: Clone + Send + Sync {
	/// Type of error this clients returns.
	type Error: Debug + MaybeConnectionError;

	/// Try to reconnect to source node.
	async fn reconnect(&mut self) -> Result<(), Self::Error>;
}

/// Run relay loop.
///
/// This function represents an outer loop, which in turn calls provided `loop_run` function to do
/// actual job. When `loop_run` returns, this outer loop reconnects to failed client (source,
/// target or both) and calls `loop_run` again.
pub fn run<SC: Client, TC: Client, R, F>(
	reconnect_delay: Duration,
	mut source_client: SC,
	mut target_client: TC,
	loop_run: R,
) where
	R: Fn(SC, TC) -> F,
	F: Future<Output = Result<(), FailedClient>>,
{
	let mut local_pool = futures::executor::LocalPool::new();

	local_pool.run_until(async move {
		loop {
			let result = loop_run(source_client.clone(), target_client.clone()).await;

			match result {
				Ok(()) => break,
				Err(failed_client) => loop {
					async_std::task::sleep(reconnect_delay).await;
					if failed_client == FailedClient::Both || failed_client == FailedClient::Source {
						match source_client.reconnect().await {
							Ok(()) => (),
							Err(error) => {
								log::warn!(
									target: "bridge",
									"Failed to reconnect to source client. Going to retry in {}s: {:?}",
									reconnect_delay.as_secs(),
									error,
								);
								continue;
							}
						}
					}
					if failed_client == FailedClient::Both || failed_client == FailedClient::Target {
						match target_client.reconnect().await {
							Ok(()) => (),
							Err(error) => {
								log::warn!(
									target: "bridge",
									"Failed to reconnect to target client. Going to retry in {}s: {:?}",
									reconnect_delay.as_secs(),
									error,
								);
								continue;
							}
						}
					}

					break;
				},
			}

			log::debug!(target: "bridge", "Restarting relay loop");
		}
	});
}
