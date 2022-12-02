// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Metrics for the statement distribution module

use polkadot_node_subsystem_util::metrics::{self, prometheus};

/// Buckets more suitable for checking the typical latency values
const HISTOGRAM_LATENCY_BUCKETS: &[f64] = &[
	0.000025, 0.00005, 0.000075, 0.0001, 0.0003125, 0.000625, 0.00125, 0.0025, 0.005, 0.01, 0.025,
	0.05, 0.1,
];

#[derive(Clone)]
struct MetricsInner {
	statements_distributed: prometheus::Counter<prometheus::U64>,
	sent_requests: prometheus::Counter<prometheus::U64>,
	received_responses: prometheus::CounterVec<prometheus::U64>,
	active_leaves_update: prometheus::Histogram,
	share: prometheus::Histogram,
	network_bridge_update_v1: prometheus::HistogramVec,
	statements_unexpected: prometheus::CounterVec<prometheus::U64>,
	created_message_size: prometheus::Gauge<prometheus::U64>,
}

/// Statement Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	/// Update statements distributed counter
	pub fn on_statement_distributed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.statements_distributed.inc();
		}
	}

	/// Update sent requests counter
	/// This counter is updated merely for the statements sent via request/response method,
	/// meaning that it counts large statements only
	pub fn on_sent_request(&self) {
		if let Some(metrics) = &self.0 {
			metrics.sent_requests.inc();
		}
	}

	/// Update counters for the received responses with `succeeded` or `failed` labels
	/// These counters are updated merely for the statements received via request/response method,
	/// meaning that they count large statements only
	pub fn on_received_response(&self, success: bool) {
		if let Some(metrics) = &self.0 {
			let label = if success { "succeeded" } else { "failed" };
			metrics.received_responses.with_label_values(&[label]).inc();
		}
	}

	/// Provide a timer for `active_leaves_update` which observes on drop.
	pub fn time_active_leaves_update(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.active_leaves_update.start_timer())
	}

	/// Provide a timer for `share` which observes on drop.
	pub fn time_share(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.share.start_timer())
	}

	/// Provide a timer for `network_bridge_update_v1` which observes on drop.
	pub fn time_network_bridge_update_v1(
		&self,
		message_type: &'static str,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| {
			metrics
				.network_bridge_update_v1
				.with_label_values(&[message_type])
				.start_timer()
		})
	}

	/// Update the out-of-view statements counter for unexpected valid statements
	pub fn on_unexpected_statement_valid(&self) {
		if let Some(metrics) = &self.0 {
			metrics.statements_unexpected.with_label_values(&["valid"]).inc();
		}
	}

	/// Update the out-of-view statements counter for unexpected seconded statements
	pub fn on_unexpected_statement_seconded(&self) {
		if let Some(metrics) = &self.0 {
			metrics.statements_unexpected.with_label_values(&["seconded"]).inc();
		}
	}

	/// Update the out-of-view statements counter for unexpected large statements
	pub fn on_unexpected_statement_large(&self) {
		if let Some(metrics) = &self.0 {
			metrics.statements_unexpected.with_label_values(&["large"]).inc();
		}
	}

	/// Report size of a created message.
	pub fn on_created_message(&self, size: usize) {
		if let Some(metrics) = &self.0 {
			metrics.created_message_size.set(size as u64);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(
		registry: &prometheus::Registry,
	) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			statements_distributed: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_statements_distributed_total",
					"Number of candidate validity statements distributed to other peers.",
				)?,
				registry,
			)?,
			sent_requests: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_statement_distribution_sent_requests_total",
					"Number of large statement fetching requests sent.",
				)?,
				registry,
			)?,
			received_responses: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_statement_distribution_received_responses_total",
						"Number of received responses for large statement data.",
					),
					&["success"],
				)?,
				registry,
			)?,
			active_leaves_update: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_statement_distribution_active_leaves_update",
						"Time spent within `statement_distribution::active_leaves_update`",
					)
					.buckets(HISTOGRAM_LATENCY_BUCKETS.into()),
				)?,
				registry,
			)?,
			share: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_statement_distribution_share",
						"Time spent within `statement_distribution::share`",
					)
					.buckets(HISTOGRAM_LATENCY_BUCKETS.into()),
				)?,
				registry,
			)?,
			network_bridge_update_v1: prometheus::register(
				prometheus::HistogramVec::new(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_statement_distribution_network_bridge_update_v1",
						"Time spent within `statement_distribution::network_bridge_update_v1`",
					)
					.buckets(HISTOGRAM_LATENCY_BUCKETS.into()),
					&["message_type"],
				)?,
				registry,
			)?,
			statements_unexpected: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_statement_distribution_statements_unexpected",
						"Number of statements that were not expected to be received.",
					),
					&["type"],
				)?,
				registry,
			)?,
			created_message_size: prometheus::register(
				prometheus::Gauge::with_opts(prometheus::Opts::new(
					"polkadot_parachain_statement_distribution_created_message_size",
					"Size of created messages containing Seconded statements.",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
