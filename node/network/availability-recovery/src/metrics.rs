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

use polkadot_node_subsystem_util::metrics::{
	self,
	prometheus::{self, Counter, CounterVec, Histogram, Opts, PrometheusError, Registry, U64},
};

/// Availability Distribution metrics.
#[derive(Clone, Default)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Clone)]
struct MetricsInner {
	/// Number of sent chunk requests.
	///
	/// Gets incremented on each sent chunk requests.
	chunk_requests_issued: Counter<U64>,

	/// A counter for finished chunk requests.
	///
	/// Split by result:
	/// - `no_such_chunk` ... peer did not have the requested chunk
	/// - `timeout` ... request timed out.
	/// - `network_error` ... Some networking issue except timeout
	/// - `invalid` ... Chunk was received, but not valid.
	/// - `success`
	chunk_requests_finished: CounterVec<U64>,

	/// The duration of request to response.
	time_chunk_request: Histogram,

	/// The duration between the pure recovery and verification.
	time_erasure_recovery: Histogram,

	/// Time of a full recovery, including erasure decoding or until we gave
	/// up.
	time_full_recovery: Histogram,

	/// Number of full recoveries that have been finished one way or the other.
	full_recoveries_finished: CounterVec<U64>,

	/// Number of full recoveries that have been started on this subsystem.
	///
	/// Note: Those are only recoveries which could not get served locally already - so in other
	/// words: Only real recoveries.
	full_recoveries_started: Counter<U64>,
}

impl Metrics {
	/// Create new dummy metrics, not reporting anything.
	pub fn new_dummy() -> Self {
		Metrics(None)
	}

	/// Increment counter on fetched labels.
	pub fn on_chunk_request_issued(&self) {
		if let Some(metrics) = &self.0 {
			metrics.chunk_requests_issued.inc()
		}
	}

	/// A chunk request timed out.
	pub fn on_chunk_request_timeout(&self) {
		if let Some(metrics) = &self.0 {
			metrics.chunk_requests_finished.with_label_values(&["timeout"]).inc()
		}
	}

	/// A chunk request failed because validator did not have its chunk.
	pub fn on_chunk_request_no_such_chunk(&self) {
		if let Some(metrics) = &self.0 {
			metrics.chunk_requests_finished.with_label_values(&["no_such_chunk"]).inc()
		}
	}

	/// A chunk request failed for some non timeout related network error.
	pub fn on_chunk_request_error(&self) {
		if let Some(metrics) = &self.0 {
			metrics.chunk_requests_finished.with_label_values(&["error"]).inc()
		}
	}

	/// A chunk request succeeded, but was not valid.
	pub fn on_chunk_request_invalid(&self) {
		if let Some(metrics) = &self.0 {
			metrics.chunk_requests_finished.with_label_values(&["invalid"]).inc()
		}
	}

	/// A chunk request succeeded.
	pub fn on_chunk_request_succeeded(&self) {
		if let Some(metrics) = &self.0 {
			metrics.chunk_requests_finished.with_label_values(&["success"]).inc()
		}
	}

	/// Get a timer to time request/response duration.
	pub fn time_chunk_request(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_chunk_request.start_timer())
	}

	/// Get a timer to time erasure code recover.
	pub fn time_erasure_recovery(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_erasure_recovery.start_timer())
	}

	/// Get a timer to measure the time of the complete recovery process.
	pub fn time_full_recovery(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_full_recovery.start_timer())
	}

	/// A full recovery succeeded.
	pub fn on_recovery_succeeded(&self) {
		if let Some(metrics) = &self.0 {
			metrics.full_recoveries_finished.with_label_values(&["success"]).inc()
		}
	}

	/// A full recovery failed (data not available).
	pub fn on_recovery_failed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.full_recoveries_finished.with_label_values(&["failure"]).inc()
		}
	}

	/// A full recovery failed (data was recovered, but invalid).
	pub fn on_recovery_invalid(&self) {
		if let Some(metrics) = &self.0 {
			metrics.full_recoveries_finished.with_label_values(&["invalid"]).inc()
		}
	}

	/// A recover was started.
	pub fn on_recovery_started(&self) {
		if let Some(metrics) = &self.0 {
			metrics.full_recoveries_started.inc()
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &Registry) -> Result<Self, PrometheusError> {
		let metrics = MetricsInner {
			chunk_requests_issued: prometheus::register(
				Counter::new(
					"polkadot_parachain_availability_recovery_chunk_requests_issued",
					"Total number of issued chunk requests.",
				)?,
				registry,
			)?,
			chunk_requests_finished: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_availability_recovery_chunk_requests_finished",
						"Total number of chunk requests finished.",
					),
					&["result"],
				)?,
				registry,
			)?,
			time_chunk_request: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_availability_recovery_time_chunk_request",
					"Time spent waiting for a response to a chunk request",
				))?,
				registry,
			)?,
			time_erasure_recovery: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_availability_recovery_time_erasure_recovery",
					"Time spent to recover the erasure code and verify the merkle root by re-encoding as erasure chunks",
				))?,
				registry,
			)?,
			time_full_recovery: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_availability_recovery_time_total",
					"Time a full recovery process took, either until failure or successful erasure decoding.",
				))?,
				registry,
			)?,
			full_recoveries_finished: prometheus::register(
				CounterVec::new(
					Opts::new(
						"polkadot_parachain_availability_recovery_recoveries_finished",
						"Total number of recoveries that finished.",
					),
					&["result"],
				)?,
				registry,
			)?,
			full_recoveries_started: prometheus::register(
				Counter::new(
					"polkadot_parachain_availability_recovery_recovieries_started",
					"Total number of started recoveries.",
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
