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

//! Runtime Metrics helpers.
//!
//! Builds on top of Substrate wasm tracing support.

use codec::Decode;
use primitives::v0::{AsStr, RuntimeMetricLabel, RuntimeMetricOp, RuntimeMetricUpdate};
use std::{
	collections::hash_map::HashMap,
	sync::{Arc, Mutex},
};
use substrate_prometheus_endpoint::{register, CounterVec, Opts, PrometheusError, Registry, U64};

/// We only support CounterVec for now.
/// TODO: add more when needed.
#[derive(Clone, Default)]
pub struct Metrics {
	counter_vecs: Arc<Mutex<HashMap<String, CounterVec<U64>>>>,
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
	pub fn register_countervec(
		&self,
		metric_name: &str,
		description: &str,
		label: RuntimeMetricLabel,
	) -> Result<(), PrometheusError> {
		if !self.1.counter_vecs.lock().expect("bad lock").contains_key(metric_name) {
			let counter_vec = register(
				CounterVec::new(
					Opts::new(metric_name, description),
					&[label.as_str().expect("invalid metric label")],
				)?,
				&self.0,
			)?;

			self.1
				.counter_vecs
				.lock()
				.expect("bad lock")
				.insert(metric_name.to_owned(), counter_vec);
		}

		Ok(())
	}

	/// Increment a counter vec by a value.
	pub fn inc_counter_by(&self, name: &str, value: u64, label: RuntimeMetricLabel) {
		match self.register_countervec(name, "default description", label.clone()) {
			Ok(_) => {
				// The metric is in the hashmap, unwrap won't panic.
				self.1
					.counter_vecs
					.lock()
					.expect("bad lock")
					.get_mut(name)
					.unwrap()
					.with_label_values(&[label.as_str().expect("invalid metric label")])
					.inc_by(value);
			},
			Err(_) => {},
		}
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

		if let Some(update_op_bs58) = event.values.string_values.get("params").cloned() {
			// TODO: Fix ugly hack because the payload comes in as a formatted string.

			// Deserialize the metric update struct.
			match RuntimeMetricUpdate::decode(
				&mut RuntimeMetricsProvider::parse_event_params(&update_op_bs58).as_ref(),
			) {
				Ok(update_op) => {
					println!("Received metric: {:?}", update_op);
					self.parse_metric_update(update_op);
				},
				Err(e) => {
					println!("Failed to decode metric: {:?}", e);
					tracing::error!("TraceEvent decode failed: {:?}", e);
				},
			}
		}
	}
}

impl RuntimeMetricsProvider {
	fn parse_metric_update(&self, update: RuntimeMetricUpdate) {
		match update.op {
			RuntimeMetricOp::Increment(value) => {
				self.inc_counter_by(update.metric_name(), value, "test_label".into());
			},
		}
	}

	// Returns the `bs58` encoded metric update operation.
	fn parse_event_params(event_params: &String) -> Vec<u8> {
		println!("params: {}", event_params);

		// Shave " }" suffix.
		let new_len = event_params.len() - 2;
		let event_params = &event_params[..new_len];

		// Shave " { update_op: " prefix.
		const SKIP_CHARS: usize = " { update_op: ".len();
		let metric_update_op = &event_params[SKIP_CHARS..];

		println!("Metric updatet op {}", metric_update_op);

		bs58::decode(metric_update_op.as_bytes()).into_vec().unwrap_or_default()
	}
}

/// Returns the custom profiling closure that we'll apply to the LoggerBuilder.
pub fn logger_hook() -> impl FnOnce(&mut sc_cli::LoggerBuilder, &sc_service::Configuration) -> () {
	|logger_builder, config| {
		if config.prometheus_registry().is_none() {
			return
		}
		let registry = config.prometheus_registry().cloned().unwrap();
		let metrics_provider = RuntimeMetricsProvider::new(registry);
		logger_builder.with_custom_profiling(Box::new(metrics_provider));
	}
}
