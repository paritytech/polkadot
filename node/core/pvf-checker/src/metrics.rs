// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Metrics definitions for the PVF pre-checking subsystem.

use polkadot_node_subsystem_util::metrics::{self, prometheus};

#[derive(Clone)]
struct MetricsInner {
	pre_check_judgement: prometheus::Histogram,
	votes_total: prometheus::Counter<prometheus::U64>,
	votes_started: prometheus::Counter<prometheus::U64>,
	votes_duplicate: prometheus::Counter<prometheus::U64>,
	pvfs_observed: prometheus::Counter<prometheus::U64>,
	pvfs_left: prometheus::Counter<prometheus::U64>,
}

#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	/// Time between sending the pre-check request to receiving the response.
	pub(crate) fn time_pre_check_judgement(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.pre_check_judgement.start_timer())
	}

	/// Called when a PVF vote/statement is submitted.
	pub(crate) fn on_vote_submitted(&self) {
		if let Some(metrics) = &self.0 {
			metrics.votes_total.inc();
		}
	}

	/// Called when a PVF vote/statement is started submission.
	pub(crate) fn on_vote_submission_started(&self) {
		if let Some(metrics) = &self.0 {
			metrics.votes_started.inc();
		}
	}

	/// Called when the vote is a duplicate.
	pub(crate) fn on_vote_duplicate(&self) {
		if let Some(metrics) = &self.0 {
			metrics.votes_duplicate.inc();
		}
	}

	/// Called when a new PVF is observed.
	pub(crate) fn on_pvf_observed(&self, num: usize) {
		if let Some(metrics) = &self.0 {
			metrics.pvfs_observed.inc_by(num as u64);
		}
	}

	/// Called when a PVF left the view.
	pub(crate) fn on_pvf_left(&self, num: usize) {
		if let Some(metrics) = &self.0 {
			metrics.pvfs_left.inc_by(num as u64);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			pre_check_judgement: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_pvf_precheck_judgement",
						"Time between sending the pre-check request to receiving the response.",
					)
					.buckets(vec![0.1, 0.5, 1.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0]),
				)?,
				registry,
			)?,
			votes_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_precheck_votes_total",
					"The total number of votes submitted.",
				)?,
				registry,
			)?,
			votes_started: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_precheck_votes_started",
					"The number of votes that are pending submission",
				)?,
				registry,
			)?,
			votes_duplicate: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_precheck_votes_duplicate",
					"The number of votes that are submitted more than once for the same code within\
the same session.",
				)?,
				registry,
			)?,
			pvfs_observed: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_precheck_pvfs_observed",
					"The number of new PVFs observed.",
				)?,
				registry,
			)?,
			pvfs_left: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_precheck_pvfs_left",
					"The number of PVFs removed from the view.",
				)?,
				registry,
			)?,
		};
		Ok(Self(Some(metrics)))
	}
}
