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

//! Prometheus metrics related to the validation host.

use polkadot_node_metrics::metrics::{self, prometheus};

/// Validation host metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	/// Returns a handle to submit prepare workers metrics.
	pub(crate) fn prepare_worker(&'_ self) -> WorkerRelatedMetrics<'_> {
		WorkerRelatedMetrics { metrics: self, flavor: WorkerFlavor::Prepare }
	}

	/// Returns a handle to submit execute workers metrics.
	pub(crate) fn execute_worker(&'_ self) -> WorkerRelatedMetrics<'_> {
		WorkerRelatedMetrics { metrics: self, flavor: WorkerFlavor::Execute }
	}

	/// When preparation pipeline had a new item enqueued.
	pub(crate) fn prepare_enqueued(&self) {
		if let Some(metrics) = &self.0 {
			metrics.prepare_enqueued.inc();
		}
	}

	/// When preparation pipeline concluded working on an item.
	pub(crate) fn prepare_concluded(&self) {
		if let Some(metrics) = &self.0 {
			metrics.prepare_concluded.inc();
		}
	}

	/// When execution pipeline had a new item enqueued.
	pub(crate) fn execute_enqueued(&self) {
		if let Some(metrics) = &self.0 {
			metrics.execute_enqueued.inc();
		}
	}

	/// When execution pipeline finished executing a request.
	pub(crate) fn execute_finished(&self) {
		if let Some(metrics) = &self.0 {
			metrics.execute_finished.inc();
		}
	}

	/// Time between sending preparation request to a worker to having the response.
	pub(crate) fn time_preparation(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.preparation_time.start_timer())
	}

	/// Time between sending execution request to a worker to having the response.
	pub(crate) fn time_execution(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.execution_time.start_timer())
	}
}

#[derive(Clone)]
struct MetricsInner {
	worker_spawning: prometheus::CounterVec<prometheus::U64>,
	worker_spawned: prometheus::CounterVec<prometheus::U64>,
	worker_retired: prometheus::CounterVec<prometheus::U64>,
	prepare_enqueued: prometheus::Counter<prometheus::U64>,
	prepare_concluded: prometheus::Counter<prometheus::U64>,
	execute_enqueued: prometheus::Counter<prometheus::U64>,
	execute_finished: prometheus::Counter<prometheus::U64>,
	preparation_time: prometheus::Histogram,
	execution_time: prometheus::Histogram,
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let inner = MetricsInner {
			worker_spawning: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_pvf_worker_spawning",
						"The total number of workers began to spawn",
					),
					&["flavor"],
				)?,
				registry,
			)?,
			worker_spawned: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_pvf_worker_spawned",
						"The total number of workers spawned successfully",
					),
					&["flavor"],
				)?,
				registry,
			)?,
			worker_retired: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_pvf_worker_retired",
						"The total number of workers retired, either killed by the host or died on duty",
					),
					&["flavor"],
				)?,
				registry,
			)?,
			prepare_enqueued: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_prepare_enqueued",
					"The total number of jobs enqueued into the preparation pipeline"
				)?,
				registry,
			)?,
			prepare_concluded: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_prepare_concluded",
					"The total number of jobs concluded in the preparation pipeline"
				)?,
				registry,
			)?,
			execute_enqueued: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_execute_enqueued",
					"The total number of jobs enqueued into the execution pipeline"
				)?,
				registry,
			)?,
			execute_finished: prometheus::register(
				prometheus::Counter::new(
					"polkadot_pvf_execute_finished",
					"The total number of jobs done in the execution pipeline"
				)?,
				registry,
			)?,
			preparation_time: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_pvf_preparation_time",
						"Time spent in preparing PVF artifacts in seconds",
					)
					.buckets(vec![
						// This is synchronized with the PRECHECK_PREPARATION_TIMEOUT=60s
						// and LENIENT_PREPARATION_TIMEOUT=360s constants found in
						// src/prepare/worker.rs
						0.1,
						0.5,
						1.0,
						2.0,
						3.0,
						10.0,
						20.0,
						30.0,
						60.0,
						120.0,
						240.0,
						360.0,
						480.0,
					]),
				)?,
				registry,
			)?,
			execution_time: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_pvf_execution_time",
						"Time spent in executing PVFs",
					).buckets(vec![
						// This is synchronized with `APPROVAL_EXECUTION_TIMEOUT`  and
						// `BACKING_EXECUTION_TIMEOUT` constants in `node/primitives/src/lib.rs`
						0.01,
						0.025,
						0.05,
						0.1,
						0.25,
						0.5,
						1.0,
						2.0,
						3.0,
						4.0,
						5.0,
						6.0,
						8.0,
						10.0,
						12.0,
					]),
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(inner)))
	}
}

enum WorkerFlavor {
	Prepare,
	Execute,
}

impl WorkerFlavor {
	fn as_label(&self) -> &'static str {
		match *self {
			WorkerFlavor::Prepare => "prepare",
			WorkerFlavor::Execute => "execute",
		}
	}
}

pub(crate) struct WorkerRelatedMetrics<'a> {
	metrics: &'a Metrics,
	flavor: WorkerFlavor,
}

impl<'a> WorkerRelatedMetrics<'a> {
	/// When the spawning of a worker started.
	pub(crate) fn on_begin_spawn(&self) {
		if let Some(metrics) = &self.metrics.0 {
			metrics.worker_spawning.with_label_values(&[self.flavor.as_label()]).inc();
		}
	}

	/// When the worker successfully spawned.
	pub(crate) fn on_spawned(&self) {
		if let Some(metrics) = &self.metrics.0 {
			metrics.worker_spawned.with_label_values(&[self.flavor.as_label()]).inc();
		}
	}

	/// When the worker was killed or died.
	pub(crate) fn on_retired(&self) {
		if let Some(metrics) = &self.metrics.0 {
			metrics.worker_retired.with_label_values(&[self.flavor.as_label()]).inc();
		}
	}
}
