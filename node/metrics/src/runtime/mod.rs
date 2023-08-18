// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Runtime Metrics helpers.
//!
//! A runtime metric provider implementation that builds on top of Substrate wasm
//! tracing support. This requires that the custom profiler (`TraceHandler`) to be
//! registered in substrate via a `logger_hook()`. Events emitted from runtime are
//! then captured/processed by the `TraceHandler` implementation.
//!
//! Don't add logs in this file because it gets executed before the logger is
//! initialized and they won't be delivered. Add println! statements if you need
//! to debug this code.

#![cfg(feature = "runtime-metrics")]

use codec::Decode;
use primitives::{
	metric_definitions::{CounterDefinition, CounterVecDefinition, HistogramDefinition},
	RuntimeMetricLabelValues, RuntimeMetricOp, RuntimeMetricUpdate,
};
use std::{
	collections::hash_map::HashMap,
	sync::{Arc, Mutex, MutexGuard},
};
use substrate_prometheus_endpoint::{
	register, Counter, CounterVec, Histogram, HistogramOpts, Opts, PrometheusError, Registry, U64,
};
mod parachain;

/// Holds the registered Prometheus metric collections.
#[derive(Clone, Default)]
pub struct Metrics {
	counter_vecs: Arc<Mutex<HashMap<String, CounterVec<U64>>>>,
	counters: Arc<Mutex<HashMap<String, Counter<U64>>>>,
	histograms: Arc<Mutex<HashMap<String, Histogram>>>,
}

/// Runtime metrics wrapper.
#[derive(Clone)]
pub struct RuntimeMetricsProvider(Registry, Metrics);

impl RuntimeMetricsProvider {
	/// Creates new instance.
	pub fn new(metrics_registry: Registry) -> Self {
		Self(metrics_registry, Metrics::default())
	}

	/// Register a counter vec metric.
	pub fn register_countervec(&self, countervec: CounterVecDefinition) {
		self.with_counter_vecs_lock_held(|mut hashmap| {
			hashmap.entry(countervec.name.to_owned()).or_insert(register(
				CounterVec::new(
					Opts::new(countervec.name, countervec.description),
					countervec.labels,
				)?,
				&self.0,
			)?);
			Ok(())
		})
	}

	/// Register a counter metric.
	pub fn register_counter(&self, counter: CounterDefinition) {
		self.with_counters_lock_held(|mut hashmap| {
			hashmap
				.entry(counter.name.to_owned())
				.or_insert(register(Counter::new(counter.name, counter.description)?, &self.0)?);
			return Ok(())
		})
	}

	/// Register a histogram metric
	pub fn register_histogram(&self, hist: HistogramDefinition) {
		self.with_histograms_lock_held(|mut hashmap| {
			hashmap.entry(hist.name.to_owned()).or_insert(register(
				Histogram::with_opts(
					HistogramOpts::new(hist.name, hist.description).buckets(hist.buckets.to_vec()),
				)?,
				&self.0,
			)?);
			return Ok(())
		})
	}

	/// Increment a counter with labels by a value
	pub fn inc_counter_vec_by(&self, name: &str, value: u64, labels: &RuntimeMetricLabelValues) {
		self.with_counter_vecs_lock_held(|mut hashmap| {
			hashmap.entry(name.to_owned()).and_modify(|counter_vec| {
				counter_vec.with_label_values(&labels.as_str_vec()).inc_by(value)
			});

			Ok(())
		});
	}

	/// Increment a counter by a value.
	pub fn inc_counter_by(&self, name: &str, value: u64) {
		self.with_counters_lock_held(|mut hashmap| {
			hashmap
				.entry(name.to_owned())
				.and_modify(|counter_vec| counter_vec.inc_by(value));
			Ok(())
		})
	}

	/// Observe a histogram. `value` should be in `ns`.
	pub fn observe_histogram(&self, name: &str, value: u128) {
		self.with_histograms_lock_held(|mut hashmap| {
			hashmap
				.entry(name.to_owned())
				.and_modify(|histogram| histogram.observe(value as f64 / 1_000_000_000.0)); // ns to sec
			Ok(())
		})
	}

	fn with_counters_lock_held<F>(&self, do_something: F)
	where
		F: FnOnce(MutexGuard<'_, HashMap<String, Counter<U64>>>) -> Result<(), PrometheusError>,
	{
		let _ = self.1.counters.lock().map(do_something).or_else(|error| Err(error));
	}

	fn with_counter_vecs_lock_held<F>(&self, do_something: F)
	where
		F: FnOnce(MutexGuard<'_, HashMap<String, CounterVec<U64>>>) -> Result<(), PrometheusError>,
	{
		let _ = self.1.counter_vecs.lock().map(do_something).or_else(|error| Err(error));
	}

	fn with_histograms_lock_held<F>(&self, do_something: F)
	where
		F: FnOnce(MutexGuard<'_, HashMap<String, Histogram>>) -> Result<(), PrometheusError>,
	{
		let _ = self.1.histograms.lock().map(do_something).or_else(|error| Err(error));
	}
}

impl sc_tracing::TraceHandler for RuntimeMetricsProvider {
	fn handle_span(&self, _span: &sc_tracing::SpanDatum) {}
	fn handle_event(&self, event: &sc_tracing::TraceEvent) {
		if event
			.values
			.string_values
			.get("target")
			.unwrap_or(&String::default())
			.ne("metrics")
		{
			return
		}

		if let Some(update_op_bs58) = event.values.string_values.get("params") {
			// Deserialize the metric update struct.
			match RuntimeMetricUpdate::decode(
				&mut RuntimeMetricsProvider::parse_event_params(&update_op_bs58)
					.unwrap_or_default()
					.as_slice(),
			) {
				Ok(update_op) => {
					self.parse_metric_update(update_op);
				},
				Err(_) => {
					// do nothing
				},
			}
		}
	}
}

impl RuntimeMetricsProvider {
	// Parse end execute the update operation.
	fn parse_metric_update(&self, update: RuntimeMetricUpdate) {
		match update.op {
			RuntimeMetricOp::IncrementCounterVec(value, ref labels) =>
				self.inc_counter_vec_by(update.metric_name(), value, labels),
			RuntimeMetricOp::IncrementCounter(value) =>
				self.inc_counter_by(update.metric_name(), value),
			RuntimeMetricOp::ObserveHistogram(value) =>
				self.observe_histogram(update.metric_name(), value),
		}
	}

	// Returns the `bs58` encoded metric update operation.
	fn parse_event_params(event_params: &str) -> Option<Vec<u8>> {
		// Shave " }" suffix.
		let new_len = event_params.len().saturating_sub(2);
		let event_params = &event_params[..new_len];

		// Shave " { update_op: " prefix.
		const SKIP_CHARS: &'static str = " { update_op: ";
		if SKIP_CHARS.len() < event_params.len() {
			if SKIP_CHARS.eq_ignore_ascii_case(&event_params[..SKIP_CHARS.len()]) {
				return bs58::decode(&event_params[SKIP_CHARS.len()..].as_bytes()).into_vec().ok()
			}
		}

		// No event was parsed
		None
	}
}

/// Returns the custom profiling closure that we'll apply to the `LoggerBuilder`.
pub fn logger_hook() -> impl FnOnce(&mut sc_cli::LoggerBuilder, &sc_service::Configuration) -> () {
	|logger_builder, config| {
		if config.prometheus_registry().is_none() {
			return
		}
		let registry = config.prometheus_registry().cloned().unwrap();
		let metrics_provider = RuntimeMetricsProvider::new(registry);
		parachain::register_metrics(&metrics_provider);
		logger_builder.with_custom_profiling(Box::new(metrics_provider));
	}
}
