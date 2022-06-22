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
	/// How long vote cleanup batches take.
	vote_cleanup_time: prometheus::Histogram,
}

/// Candidate validation metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
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

	pub(crate) fn time_vote_cleanup(&self) -> Option<prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.vote_cleanup_time.start_timer())
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
			vote_cleanup_time: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_dispute_coordinator_vote_cleanup",
						"Time spent cleaning up old votes per batch.",
					)
					.buckets([0.01, 0.1, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0].into()),
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
