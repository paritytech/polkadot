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
#![cfg(feature = "with-tracing")]

use codec::Decode;
use primitives::v1::{
	RuntimeMetricLabelValues, RuntimeMetricOp, RuntimeMetricRegisterParams, RuntimeMetricUpdate,
};
use std::{
	collections::hash_map::HashMap,
	sync::{Arc, Mutex},
};
use substrate_prometheus_endpoint::{register, CounterVec, Opts, PrometheusError, Registry, U64};

const LOG_TARGET: &'static str = "metrics::runtime";

/// Support only CounterVec for now.
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
		params: &RuntimeMetricRegisterParams,
	) -> Result<(), PrometheusError> {
		match self.1.counter_vecs.lock() {
			Ok(mut unlocked_hashtable) => {
				if unlocked_hashtable.contains_key(metric_name) {
					return Ok(())
				}

				unlocked_hashtable.insert(
					metric_name.to_owned(),
					register(
						CounterVec::new(
							Opts::new(metric_name, params.description()),
							&params.labels(),
						)?,
						&self.0,
					)?,
				);
			},
			Err(e) => tracing::error!(
				target: LOG_TARGET,
				"Failed to acquire the `counter_vecs` lock: {:?}",
				e
			),
		}
		Ok(())
	}

	/// Increment a counter vec by a value.
	pub fn inc_counter_by(&self, name: &str, value: u64, labels: &RuntimeMetricLabelValues) {
		let _ = self.1.counter_vecs.lock().map(|mut unlocked_hashtable| {
			if let Some(counter_vec) = unlocked_hashtable.get_mut(name) {
				counter_vec.with_label_values(&labels.as_str()).inc_by(value);
			} else {
				tracing::error!(
					target: LOG_TARGET,
					"Cannot increment counter `{}`, metric not in registered or present in hashtable",
					name
				);
			}
		});
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
			// Deserialize the metric update struct.
			match RuntimeMetricUpdate::decode(
				&mut RuntimeMetricsProvider::parse_event_params(&update_op_bs58)
					.unwrap_or_default()
					.as_slice(),
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
	// Parse end execute the update operation.
	fn parse_metric_update(&self, update: RuntimeMetricUpdate) {
		match update.op {
			RuntimeMetricOp::Register(ref params) => {
				let _ = self.register_countervec(update.metric_name(), &params);
			},
			RuntimeMetricOp::Increment(value, ref labels) =>
				self.inc_counter_by(update.metric_name(), value, labels),
		}
	}

	// Returns the `bs58` encoded metric update operation.
	fn parse_event_params(event_params: &String) -> Option<Vec<u8>> {
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
