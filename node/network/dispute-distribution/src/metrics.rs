// Copyright 2021 Parity Technologies (UK) Ltd.
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

use polkadot_node_subsystem_util::{
	metrics,
	metrics::{
		prometheus,
		prometheus::{Counter, CounterVec, Opts, PrometheusError, Registry, U64},
	},
};

/// Label for success counters.
pub const SUCCEEDED: &'static str = "succeeded";

/// Label for fail counters.
pub const FAILED: &'static str = "failed";

/// Dispute Distribution metrics.
#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Clone)]
struct MetricsInner {
	/// Number of sent dispute requests (succeeded and failed).
	sent_requests: CounterVec<U64>,

	/// Number of requests received.
	///
	/// This is all requests coming in, regardless of whether they are processed or dropped.
	received_requests: Counter<U64>,

	/// Number of requests for which `ImportStatements` returned.
	///
	/// We both have successful imports and failed imports here.
	imported_requests: CounterVec<U64>,

	/// The duration of issued dispute request to response.
	time_dispute_request: prometheus::Histogram,
}

impl Metrics {
	/// Create new dummy metrics, not reporting anything.
	pub fn new_dummy() -> Self {
		Metrics(None)
	}

	/// Increment counter on finished request sending.
	pub fn on_sent_request(&self, label: &'static str) {
		if let Some(metrics) = &self.0 {
			metrics.sent_requests.with_label_values(&[label]).inc()
		}
	}

	/// Increment counter on served disputes.
	pub fn on_received_request(&self) {
		if let Some(metrics) = &self.0 {
			metrics.received_requests.inc()
		}
	}

	/// Statements have been imported.
	pub fn on_imported(&self, label: &'static str, num_requests: usize) {
		if let Some(metrics) = &self.0 {
			metrics
				.imported_requests
				.with_label_values(&[label])
				.inc_by(num_requests as u64)
		}
	}

	/// Get a timer to time request/response duration.
	pub fn time_dispute_request(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_dispute_request.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MetricsInner {
			sent_requests: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_dispute_distribution_sent_requests",
						"Total number of sent requests.",
					),
					&["success"],
				)?,
				registry,
			)?,
			received_requests: prometheus::register(
				Counter::new(
					"polkadot_parachain_dispute_distribution_received_requests",
					"Total number of received dispute requests.",
				)?,
				registry,
			)?,
			imported_requests: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_dispute_distribution_imported_requests",
						"Total number of imported requests.",
					),
					&["success"],
				)?,
				registry,
			)?,
			time_dispute_request: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_dispute_distribution_time_dispute_request",
					"Time needed for dispute votes to get confirmed/fail getting transmitted.",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
