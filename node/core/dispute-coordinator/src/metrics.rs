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

use polkadot_node_subsystem_util::metrics::{self, prometheus};

#[derive(Clone)]
struct MetricsInner {
	/// Number of opened disputes.
	open: prometheus::Counter<prometheus::U64>,
	/// Votes of all disputes.
	votes: prometheus::CounterVec<prometheus::U64>,
	/// Conclusion across all disputes.
	concluded: prometheus::CounterVec<prometheus::U64>,
	/// Number of participations that have been queued.
	queued_participations: prometheus::CounterVec<prometheus::U64>,
	/// Number and duration of requests handled by kind
	requests: prometheus::HistogramVec,
	/// Time it takes to transmit writes to db
	db_operations: prometheus::HistogramVec,
}

/// Candidate validation metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	#[cfg(test)]
	pub(crate) fn new_dummy() -> Self {
		Self(None)
	}

	pub(crate) fn on_open(&self) {
		if let Some(metrics) = &self.0 {
			metrics.open.inc();
		}
	}

	pub(crate) fn on_valid_vote(&self) {
		if let Some(metrics) = &self.0 {
			metrics.votes.with_label_values(&["valid"]).inc();
		}
	}

	pub(crate) fn on_invalid_vote(&self) {
		if let Some(metrics) = &self.0 {
			metrics.votes.with_label_values(&["invalid"]).inc();
		}
	}

	pub(crate) fn on_concluded_valid(&self) {
		if let Some(metrics) = &self.0 {
			metrics.concluded.with_label_values(&["valid"]).inc();
		}
	}

	pub(crate) fn on_concluded_invalid(&self) {
		if let Some(metrics) = &self.0 {
			metrics.concluded.with_label_values(&["invalid"]).inc();
		}
	}

	pub(crate) fn on_queued_priority_participation(&self) {
		if let Some(metrics) = &self.0 {
			metrics.queued_participations.with_label_values(&["priority"]).inc();
		}
	}

	pub(crate) fn on_queued_best_effort_participation(&self) {
		if let Some(metrics) = &self.0 {
			metrics.queued_participations.with_label_values(&["best-effort"]).inc();
		}
	}

	/// Time a particular request.
	pub fn time_request(
		&self,
		req: &'static str,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.requests.with_label_values(&[req]).start_timer())
	}

	/// Time a DB write operation.
	pub fn time_db_write_operation(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| {
			metrics.db_operations.with_label_values(&["write", "flush"]).start_timer()
		})
	}

	/// Time a DB read operation.
	pub fn time_db_read_operation(
		&self,
		kind: &str,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.db_operations.with_label_values(&["read", kind]).start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			open: prometheus::register(
				prometheus::Counter::with_opts(prometheus::Opts::new(
					"polkadot_parachain_candidate_disputes_total",
					"Total number of raised disputes.",
				))?,
				registry,
			)?,
			concluded: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_candidate_dispute_concluded",
						"Concluded dispute votes, sorted by candidate is `valid` and `invalid`.",
					),
					&["validity"],
				)?,
				registry,
			)?,
			votes: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_candidate_dispute_votes",
						"Accumulated dispute votes, sorted by candidate is `valid` and `invalid`.",
					),
					&["validity"],
				)?,
				registry,
			)?,
			queued_participations: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_dispute_participations",
						"Total number of queued participations, grouped by priority and best-effort. (Not every queueing will necessarily lead to an actual participation because of duplicates.)",
					),
					&["priority"],
				)?,
				registry,
			)?,
			requests: prometheus::register(
				prometheus::HistogramVec::new(
					prometheus::HistogramOpts::new(
						"polkadot_dispute_coordinator_requests_time",
						"Number and duration of handled requests in dispute coordinator.",
					),
					&["request"],
				)?,
				registry,
			)?,
			db_operations: prometheus::register(
				prometheus::HistogramVec::new(
					prometheus::HistogramOpts::new(
						"polkadot_dispute_coordinator_db_operation_time",
						"Duration a db operation takes.",
					),
					&["operation", "kind"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
