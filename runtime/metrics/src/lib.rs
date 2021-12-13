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

#[cfg(not(feature = "std"))]
use parity_scale_codec::Encode;

#[cfg(not(feature = "std"))]
use primitives::v1::{
	RuntimeMetricLabelValues, RuntimeMetricOp, RuntimeMetricRegisterParams, RuntimeMetricUpdate,
};

#[cfg(not(feature = "std"))]
pub struct CounterVec {
	name: &'static str,
	label_values: Option<RuntimeMetricLabelValues>,
}

#[cfg(not(feature = "std"))]
impl CounterVec {
	/// Create a new counter metric.
	pub fn new(name: &'static str, description: &'static str) -> Self {
		Self::new_with_labels(name, description, &sp_std::vec::Vec::new())
	}

	/// Create a new counter metric with specified `labels`.
	pub fn new_with_labels(
		name: &'static str,
		description: &'static str,
		labels: &[&'static str],
	) -> Self {
		// Send a register metric operation to node side.
		let metric_update = RuntimeMetricUpdate {
			metric_name: sp_std::vec::Vec::from(name),
			op: RuntimeMetricOp::Register(RuntimeMetricRegisterParams::new(
				sp_std::vec::Vec::from(description),
				Some(labels.into()),
			)),
		}
		.encode();

		// `from_utf8_unchecked` is safe in this context:
		// We cannot guarantee that the input slice is a valid UTF-8, but the `bs58` encoding
		// provides that guarantee forward. We just need to `transmute` as the layout of `&str`
    	// and `&[u8]` is the same.
		unsafe {
			let register_metric_op =
				bs58::encode(sp_std::str::from_utf8_unchecked(&metric_update)).into_string();

			sp_tracing::event!(
				target: "metrics",
				sp_tracing::Level::TRACE,
				update_op = register_metric_op.as_str()
			);
		}

		CounterVec { name, label_values: None }
	}

	/// Set label values.
	pub fn with_label_values(&mut self, label_values: &[&'static str]) -> &mut Self {
		self.label_values = Some(label_values.into());
		self
	}

	/// Increment by `value`.
	pub fn inc_by(&mut self, value: u64) {
		let metric_update = RuntimeMetricUpdate {
			metric_name: sp_std::vec::Vec::from(self.name),
			op: RuntimeMetricOp::Increment(value, self.label_values.take()),
		}
		.encode();

		// `from_utf8_unchecked` is safe in this context:
		// We cannot guarantee that the input slice is a valid UTF-8, but the `bs58` encoding
		// provides that guarantee forward. We just need to `transmute` as the layout of `&str`
    	// and `&[u8]` is the same.
		unsafe {
			let update_op =
				bs58::encode(sp_std::str::from_utf8_unchecked(&metric_update)).into_string();

			sp_tracing::event!(
				target: "metrics",
				sp_tracing::Level::TRACE,
				update_op = update_op.as_str()
			);
		}
	}
}

#[cfg(feature = "std")]
pub struct CounterVec;

/// Dummy implementation.
#[cfg(feature = "std")]
impl CounterVec {
	pub fn new(_name: &'static str, _description: &'static str) -> Self {
		CounterVec
	}
	pub fn new_with_labels(
		_name: &'static str,
		_description: &'static str,
		_labels: &[&'static str],
	) -> Self {
		CounterVec
	}

	pub fn with_label_values(&mut self, _label_values: &[&'static str]) -> &mut Self {
		self
	}

	pub fn inc_by(&mut self, _: u64) {}
}
