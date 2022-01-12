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

/// Label for chunks/PoVs that could not be served, because they were not available.
pub const NOT_FOUND: &'static str = "not-found";

/// Availability Distribution metrics.
#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Clone)]
struct MetricsInner {
	/// Number of chunks fetched.
	///
	/// Note: The failed count gets incremented, when we were not able to fetch the chunk at all.
	/// For times, where we failed downloading, but succeeded on the next try (with different
	/// backers), see `retries`.
	fetched_chunks: CounterVec<U64>,

	/// Number of chunks served.
	served_chunks: CounterVec<U64>,

	/// Number of received fetch PoV responses.
	fetched_povs: CounterVec<U64>,

	/// Number of PoVs served.
	served_povs: CounterVec<U64>,

	/// Number of times our first set of validators did not provide the needed chunk and we had to
	/// query further validators.
	retries: Counter<U64>,
}

impl Metrics {
	/// Create new dummy metrics, not reporting anything.
	pub fn new_dummy() -> Self {
		Metrics(None)
	}

	/// Increment counter on fetched labels.
	pub fn on_fetch(&self, label: &'static str) {
		if let Some(metrics) = &self.0 {
			metrics.fetched_chunks.with_label_values(&[label]).inc()
		}
	}

	/// Increment counter on served chunks.
	pub fn on_served_chunk(&self, label: &'static str) {
		if let Some(metrics) = &self.0 {
			metrics.served_chunks.with_label_values(&[label]).inc()
		}
	}

	/// Increment counter on fetched PoVs.
	pub fn on_fetched_pov(&self, label: &'static str) {
		if let Some(metrics) = &self.0 {
			metrics.fetched_povs.with_label_values(&[label]).inc()
		}
	}

	/// Increment counter on served PoVs.
	pub fn on_served_pov(&self, label: &'static str) {
		if let Some(metrics) = &self.0 {
			metrics.served_povs.with_label_values(&[label]).inc()
		}
	}

	/// Increment retry counter.
	pub fn on_retry(&self) {
		if let Some(metrics) = &self.0 {
			metrics.retries.inc()
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MetricsInner {
			fetched_chunks: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_fetched_chunks_total",
						"Total number of fetched chunks.",
					),
					&["success"]
				)?,
				registry,
			)?,
			served_chunks: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_served_chunks_total",
						"Total number of chunks served by this backer.",
					),
					&["success"]
				)?,
				registry,
			)?,
			fetched_povs: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_fetched_povs_total",
						"Total number of povs fetches by this backer.",
					),
					&["success"]
				)?,
				registry,
			)?,
			served_povs: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_served_povs_total",
						"Total number of povs served by this backer.",
					),
					&["success"]
				)?,
				registry,
			)?,
			retries: prometheus::register(
				Counter::new(
					"polkadot_parachain_fetch_retries_total",
					"Number of times we did not succeed in fetching a chunk and needed to try more backers.",
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
