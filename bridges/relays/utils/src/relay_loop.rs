// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

use crate::{
	error::Error,
	metrics::{Metric, MetricsAddress, MetricsParams},
	FailedClient, MaybeConnectionError,
};

use async_trait::async_trait;
use std::{fmt::Debug, future::Future, net::SocketAddr, time::Duration};
use substrate_prometheus_endpoint::{init_prometheus, Registry};

/// Default pause between reconnect attempts.
pub const RECONNECT_DELAY: Duration = Duration::from_secs(10);

/// Basic blockchain client from relay perspective.
#[async_trait]
pub trait Client: 'static + Clone + Send + Sync {
	/// Type of error these clients returns.
	type Error: 'static + Debug + MaybeConnectionError + Send + Sync;

	/// Try to reconnect to source node.
	async fn reconnect(&mut self) -> Result<(), Self::Error>;
}

#[async_trait]
impl Client for () {
	type Error = crate::StringifiedMaybeConnectionError;

	async fn reconnect(&mut self) -> Result<(), Self::Error> {
		Ok(())
	}
}

/// Returns generic loop that may be customized and started.
pub fn relay_loop<SC, TC>(source_client: SC, target_client: TC) -> Loop<SC, TC, ()> {
	Loop { reconnect_delay: RECONNECT_DELAY, source_client, target_client, loop_metric: None }
}

/// Returns generic relay loop metrics that may be customized and used in one or several relay
/// loops.
pub fn relay_metrics(params: MetricsParams) -> LoopMetrics<(), (), ()> {
	LoopMetrics {
		relay_loop: Loop {
			reconnect_delay: RECONNECT_DELAY,
			source_client: (),
			target_client: (),
			loop_metric: None,
		},
		address: params.address,
		registry: params.registry,
		loop_metric: None,
	}
}

/// Generic relay loop.
pub struct Loop<SC, TC, LM> {
	reconnect_delay: Duration,
	source_client: SC,
	target_client: TC,
	loop_metric: Option<LM>,
}

/// Relay loop metrics builder.
pub struct LoopMetrics<SC, TC, LM> {
	relay_loop: Loop<SC, TC, ()>,
	address: Option<MetricsAddress>,
	registry: Registry,
	loop_metric: Option<LM>,
}

impl<SC, TC, LM> Loop<SC, TC, LM> {
	/// Customize delay between reconnect attempts.
	pub fn reconnect_delay(mut self, reconnect_delay: Duration) -> Self {
		self.reconnect_delay = reconnect_delay;
		self
	}

	/// Start building loop metrics using given prefix.
	pub fn with_metrics(self, params: MetricsParams) -> LoopMetrics<SC, TC, ()> {
		LoopMetrics {
			relay_loop: Loop {
				reconnect_delay: self.reconnect_delay,
				source_client: self.source_client,
				target_client: self.target_client,
				loop_metric: None,
			},
			address: params.address,
			registry: params.registry,
			loop_metric: None,
		}
	}

	/// Run relay loop.
	///
	/// This function represents an outer loop, which in turn calls provided `run_loop` function to
	/// do actual job. When `run_loop` returns, this outer loop reconnects to failed client (source,
	/// target or both) and calls `run_loop` again.
	pub async fn run<R, F>(mut self, loop_name: String, run_loop: R) -> Result<(), Error>
	where
		R: 'static + Send + Fn(SC, TC, Option<LM>) -> F,
		F: 'static + Send + Future<Output = Result<(), FailedClient>>,
		SC: 'static + Client,
		TC: 'static + Client,
		LM: 'static + Send + Clone,
	{
		let run_loop_task = async move {
			crate::initialize::initialize_loop(loop_name);

			loop {
				let loop_metric = self.loop_metric.clone();
				let future_result =
					run_loop(self.source_client.clone(), self.target_client.clone(), loop_metric);
				let result = future_result.await;

				match result {
					Ok(()) => break,
					Err(failed_client) =>
						reconnect_failed_client(
							failed_client,
							self.reconnect_delay,
							&mut self.source_client,
							&mut self.target_client,
						)
						.await,
				}

				log::debug!(target: "bridge", "Restarting relay loop");
			}

			Ok(())
		};

		async_std::task::spawn(run_loop_task).await
	}
}

impl<SC, TC, LM> LoopMetrics<SC, TC, LM> {
	/// Add relay loop metrics.
	///
	/// Loop metrics will be passed to the loop callback.
	pub fn loop_metric<NewLM: Metric>(
		self,
		metric: NewLM,
	) -> Result<LoopMetrics<SC, TC, NewLM>, Error> {
		metric.register(&self.registry)?;

		Ok(LoopMetrics {
			relay_loop: self.relay_loop,
			address: self.address,
			registry: self.registry,
			loop_metric: Some(metric),
		})
	}

	/// Convert into `MetricsParams` structure so that metrics registry may be extended later.
	pub fn into_params(self) -> MetricsParams {
		MetricsParams { address: self.address, registry: self.registry }
	}

	/// Expose metrics using address passed at creation.
	///
	/// If passed `address` is `None`, metrics are not exposed.
	pub async fn expose(self) -> Result<Loop<SC, TC, LM>, Error> {
		if let Some(address) = self.address {
			let socket_addr = SocketAddr::new(
				address
					.host
					.parse()
					.map_err(|err| Error::ExposingMetricsInvalidHost(address.host.clone(), err))?,
				address.port,
			);

			let registry = self.registry;
			async_std::task::spawn(async move {
				let runtime =
					match tokio::runtime::Builder::new_current_thread().enable_all().build() {
						Ok(runtime) => runtime,
						Err(err) => {
							log::trace!(
								target: "bridge-metrics",
								"Failed to create tokio runtime. Prometheus meterics are not available: {:?}",
								err,
							);
							return
						},
					};

				let _ = runtime.block_on(async move {
					log::trace!(
						target: "bridge-metrics",
						"Starting prometheus endpoint at: {:?}",
						socket_addr,
					);
					let result = init_prometheus(socket_addr, registry).await;
					log::trace!(
						target: "bridge-metrics",
						"Prometheus endpoint has exited with result: {:?}",
						result,
					);
				});
			});
		}

		Ok(Loop {
			reconnect_delay: self.relay_loop.reconnect_delay,
			source_client: self.relay_loop.source_client,
			target_client: self.relay_loop.target_client,
			loop_metric: self.loop_metric,
		})
	}
}

/// Deal with the client who has returned connection error.
pub async fn reconnect_failed_client(
	failed_client: FailedClient,
	reconnect_delay: Duration,
	source_client: &mut impl Client,
	target_client: &mut impl Client,
) {
	loop {
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
					continue
				},
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
					continue
				},
			}
		}

		break
	}
}
