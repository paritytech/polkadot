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
//! Each instance of a runtime metric is mapped to a Prometheus metric on the client side.
//! The runtime metrics must be registered with the registry in the client, otherwise
//! they will not be published.

const TRACING_TARGET: &'static str = "metrics";

use parity_scale_codec::Encode;
use primitives::v1::{
	metric_definitions::{CounterDefinition, CounterVecDefinition},
	RuntimeMetricLabelValues, RuntimeMetricOp, RuntimeMetricUpdate,
};

use sp_std::prelude::*;

/// Holds a set of counters that have different values for their labels,
/// like Prometheus `CounterVec`.
pub struct CounterVec {
	name: &'static str,
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

///
pub struct LabeledMetric {
	name: &'static str,
	label_values: RuntimeMetricLabelValues,
}

impl LabeledMetric {
	/// Increment the counter by `value`.
	pub fn inc_by(&self, value: u64) {
		let metric_update = RuntimeMetricUpdate {
			metric_name: Vec::from(self.name),
			op: RuntimeMetricOp::IncrementCounterVec(value, self.label_values.clone()),
		};

		Self::emit(&metric_update);
	}

	/// Increment the counter value.
	pub fn inc(&self) {
		self.inc_by(1);
	}
}

impl MetricEmitter for LabeledMetric {}
impl MetricEmitter for Counter {}

impl CounterVec {
	/// Create a new counter as specified by `definition`. This metric needs to be registered
	/// in the client before it can be used.
	pub const fn new(definition: CounterVecDefinition) -> Self {
		// No register op is emitted since the metric is supposed to be registered
		// on the client by the time `inc()` is called.
		CounterVec { name: definition.name }
	}

	/// Returns a `LabeledMetric` instance that provides an interface for incrementing
	/// the metric.
	pub fn with_label_values(&self, label_values: &[&'static str]) -> LabeledMetric {
		LabeledMetric { name: self.name, label_values: label_values.into() }
	}
}

impl Counter {
	/// Create a new counter as specified by `definition`. This metric needs to be registered
	/// in the client before it can be used.
	pub const fn new(definition: CounterDefinition) -> Self {
		Counter { name: definition.name }
	}

	/// Increment counter by `value`.
	pub fn inc_by(&self, value: u64) {
		let metric_update = RuntimeMetricUpdate {
			metric_name: Vec::from(self.name),
			op: RuntimeMetricOp::IncrementCounter(value),
		};

		Self::emit(&metric_update);
	}

	/// Increment counter.
	pub fn inc(&self) {
		self.inc_by(1);
	}
}
