// Copyright 2017-2023 Parity Technologies (UK) Ltd.
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

use polkadot_node_subsystem_util::metrics::{self, prometheus};

#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub fn on_request(&self, succeeded: std::result::Result<(), ()>) {
		if let Some(metrics) = &self.0 {
			match succeeded {
				Ok(()) => metrics.collation_requests.with_label_values(&["succeeded"]).inc(),
				Err(()) => metrics.collation_requests.with_label_values(&["failed"]).inc(),
			}
		}
	}

	/// Provide a timer for `process_msg` which observes on drop.
	pub fn time_process_msg(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_msg.start_timer())
	}

	/// Provide a timer for `handle_collation_request_result` which observes on drop.
	pub fn time_handle_collation_request_result(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.handle_collation_request_result.start_timer())
	}

	/// Note the current number of collator peers.
	pub fn note_collator_peer_count(&self, collator_peers: usize) {
		self.0
			.as_ref()
			.map(|metrics| metrics.collator_peer_count.set(collator_peers as u64));
	}

	/// Provide a timer for `CollationFetchRequest` structure which observes on drop.
	pub fn time_collation_request_duration(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.collation_request_duration.start_timer())
	}

	/// Provide a timer for `request_unblocked_collations` which observes on drop.
	pub fn time_request_unblocked_collations(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.request_unblocked_collations.start_timer())
	}
}

#[derive(Clone)]
struct MetricsInner {
	collation_requests: prometheus::CounterVec<prometheus::U64>,
	process_msg: prometheus::Histogram,
	handle_collation_request_result: prometheus::Histogram,
	collator_peer_count: prometheus::Gauge<prometheus::U64>,
	collation_request_duration: prometheus::Histogram,
	request_unblocked_collations: prometheus::Histogram,
}

impl metrics::Metrics for Metrics {
	fn try_register(
		registry: &prometheus::Registry,
	) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			collation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_collation_requests_total",
						"Number of collations requested from Collators.",
					),
					&["success"],
				)?,
				registry,
			)?,
			process_msg: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_process_msg",
						"Time spent within `collator_protocol_validator::process_msg`",
					)
				)?,
				registry,
			)?,
			handle_collation_request_result: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_handle_collation_request_result",
						"Time spent within `collator_protocol_validator::handle_collation_request_result`",
					)
				)?,
				registry,
			)?,
			collator_peer_count: prometheus::register(
				prometheus::Gauge::new(
					"polkadot_parachain_collator_peer_count",
					"Amount of collator peers connected",
				)?,
				registry,
			)?,
			collation_request_duration: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_collation_request_duration",
						"Lifetime of the `CollationFetchRequest` structure",
					).buckets(vec![0.05, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0, 1.2, 1.5, 1.75]),
				)?,
				registry,
			)?,
			request_unblocked_collations: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_collator_protocol_validator_request_unblocked_collations",
						"Time spent within `collator_protocol_validator::request_unblocked_collations`",
					)
				)?,
				registry,
			)?,
		};

		Ok(Metrics(Some(metrics)))
	}
}
