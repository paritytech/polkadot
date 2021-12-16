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

//! This module provides an implementation for the runtime metrics types: `Counter`
//! and `CounterVec`. These types expose a Prometheus like interface and same functionality.
//! Each instance of a runtime metric is mapped to a Prometheus metric on the node side.

const TRACING_TARGET: &'static str = "metrics";

use parity_scale_codec::Encode;
use primitives::v1::{
	RuntimeMetricLabelValues, RuntimeMetricOp, RuntimeMetricRegisterParams, RuntimeMetricUpdate,
};
use sp_std::prelude::*;

/// Holds a set of counters that have different values for their labels,
/// like Prometheus CounterVec.
pub struct CounterVec {
	name: &'static str,
	label_values: Option<RuntimeMetricLabelValues>,
}

/// A counter metric.
pub struct Counter {
	name: &'static str,
}

/// Convenience trait implemented for all metric types.
trait MetricEmitter {
	fn emit(metric_op: &RuntimeMetricUpdate) {
		sp_tracing::event!(
			target: TRACING_TARGET,
			sp_tracing::Level::TRACE,
			update_op = bs58::encode(&metric_op.encode()).into_string().as_str()
		);
	}
}

impl MetricEmitter for CounterVec {}
impl MetricEmitter for Counter {}

impl CounterVec {
	/// Create a new metric with specified `name`, `description` and `labels`.
	pub fn new(name: &'static str, description: &'static str, labels: &[&'static str]) -> Self {
		// Send a register metric operation to node side.
		let metric_update = RuntimeMetricUpdate {
			metric_name: Vec::from(name),
			op: RuntimeMetricOp::Register(RuntimeMetricRegisterParams::new(
				Vec::from(description),
				Some(labels.into()),
			)),
		};

		Self::emit(&metric_update);

		CounterVec { name, label_values: None }
	}

	/// Set the label values. Must be called before each increment operation.
	pub fn with_label_values(&mut self, label_values: &[&'static str]) -> &mut Self {
		self.label_values = Some(label_values.into());
		self
	}

	/// Increment the counter by `value`.
	pub fn inc_by(&mut self, value: u64) {
		self.label_values.take().map(|label_values| {
			let metric_update = RuntimeMetricUpdate {
				metric_name: Vec::from(self.name),
				op: RuntimeMetricOp::IncrementCounterVec(value, label_values),
			};

			Self::emit(&metric_update);
		});
	}

	/// Increment the counter value.
	pub fn inc(&mut self) {
		self.inc_by(1);
	}
}

impl Counter {
	/// Create a new counter metric with specified `name`, `description`.
	pub fn new(name: &'static str, description: &'static str) -> Self {
		// Send a register metric operation to node side.
		let metric_update = RuntimeMetricUpdate {
			metric_name: Vec::from(name),
			op: RuntimeMetricOp::Register(RuntimeMetricRegisterParams::new(
				Vec::from(description),
				None,
			)),
		};

		Self::emit(&metric_update);
		Counter { name }
	}

	/// Increment counter by `value`.
	pub fn inc_by(&mut self, value: u64) {
		let metric_update = RuntimeMetricUpdate {
			metric_name: Vec::from(self.name),
			op: RuntimeMetricOp::IncrementCounter(value),
		};

		Self::emit(&metric_update);
	}

	/// Increment counter.
	pub fn inc(&mut self) {
		self.inc_by(1);
	}
}
