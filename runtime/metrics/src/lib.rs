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

//! Runtime metric interface similar to native Prometheus metrics.
//! 
//! This is intended to be used only for testing and debugging and **must never 
//! be used in production**. It requires the Substrate wasm tracing support
//! and command line configuration: `--tracing-targets wasm_tracing=trace`.

#![cfg_attr(not(feature = "std"), no_std)]
use primitives::v1::{RuntimeMetricLabelValues, RuntimeMetricLabels};

#[cfg(not(feature = "std"))]
use parity_scale_codec::Encode;

#[cfg(not(feature = "std"))]
use primitives::v1::{RuntimeMetricOp, RuntimeMetricUpdate};

#[cfg(not(feature = "std"))]
pub struct CounterVec {
	name: &'static str,
	labels: RuntimeMetricLabels,
	label_values: RuntimeMetricLabelValues,
}

#[cfg(not(feature = "std"))]
impl CounterVec {
	/// Create a new counter metric.
	pub fn new(name: &'static str) -> Self {
		Self::new_with_labels(name, sp_std::vec::Vec::new())
	}

	/// Create a new counter metric with specified `labels`.
	pub fn new_with_labels(name: &'static str, labels: RuntimeMetricLabels) -> Self {
		CounterVec { name, labels, label_values: RuntimeMetricLabelValues::default() }
	}

	/// Set label values.
	pub fn with_label_values(&mut self, label_values: RuntimeMetricLabelValues) -> &Self {
		self.label_values = label_values;
		self
	}

	/// Increment by `value`.
	pub fn inc_by(&mut self, value: u64) {
		let metric_update = RuntimeMetricUpdate {
			metric_name: sp_std::vec::Vec::from(self.name),
			op: RuntimeMetricOp::Increment(value),
		}
		.encode();

		// `from_utf8_unchecked` is safe, we only care about the `metric_name` which is static str.
		unsafe {
			let update_op =
				bs58::encode(sp_std::str::from_utf8_unchecked(&metric_update)).into_string();

			sp_tracing::event!(
				target: "metrics",
				sp_tracing::Level::TRACE,
				update_op = update_op.as_str()
			);
		}

		self.label_values.clear();
	}
}

#[cfg(feature = "std")]
pub struct CounterVec;

/// Dummy implementation.
#[cfg(feature = "std")]
impl CounterVec {
	pub fn new(_: &'static str) -> Self {
		CounterVec
	}
	pub fn new_with_labels(_: &'static str, _: RuntimeMetricLabels) -> Self {
		CounterVec
	}

	pub fn with_label_values(&mut self, _: RuntimeMetricLabelValues) -> &Self {
		self
	}

	pub fn inc_by(&mut self, _: u64) {}
}
