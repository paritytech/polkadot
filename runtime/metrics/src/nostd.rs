#![cfg_attr(not(feature = "std"), no_std)]

const TRACING_TARGET: &'static str = "metrics";

use parity_scale_codec::Encode;
use primitives::v1::{
	RuntimeMetricLabelValues, RuntimeMetricOp, RuntimeMetricRegisterParams, RuntimeMetricUpdate,
};

pub struct CounterVec {
	name: &'static str,
	label_values: Option<RuntimeMetricLabelValues>,
}

pub struct Counter {
	name: &'static str,
}

fn emit_metric(metric_op: &[u8]) {
	// `from_utf8_unchecked` is safe in this context:
	// We cannot guarantee that the input slice is a valid UTF-8, but the `bs58` encoding
	// provides that guarantee forward. We just need to `transmute` as the layout of `&str`
	// and `&[u8]` is the same.

	unsafe {
		let metric_op = bs58::encode(sp_std::str::from_utf8_unchecked(&metric_op)).into_string();

		sp_tracing::event!(
			target: TRACING_TARGET,
			sp_tracing::Level::TRACE,
			update_op = metric_op.as_str()
		);
	}
}

impl CounterVec {
	/// Create a new counter metric with specified `labels`.
	pub fn new(name: &'static str, description: &'static str, labels: &[&'static str]) -> Self {
		// Send a register metric operation to node side.
		let metric_update = RuntimeMetricUpdate {
			metric_name: sp_std::vec::Vec::from(name),
			op: RuntimeMetricOp::Register(RuntimeMetricRegisterParams::new(
				sp_std::vec::Vec::from(description),
				Some(labels.into()),
			)),
		}
		.encode();

		emit_metric(&metric_update);

		CounterVec { name, label_values: None }
	}

	/// Set label values.
	pub fn with_label_values(&mut self, label_values: &[&'static str]) -> &mut Self {
		self.label_values = Some(label_values.into());
		self
	}

	/// Increment by `value`.
	pub fn inc_by(&mut self, value: u64) {
		self.label_values.take().map(|label_values| {
			let metric_update = RuntimeMetricUpdate {
				metric_name: sp_std::vec::Vec::from(self.name),
				op: RuntimeMetricOp::IncrementCounterVec(value, label_values),
			}
			.encode();

			emit_metric(&metric_update);
		});
	}

	/// Increment by 1.
	pub fn inc(&mut self) {
		self.inc_by(1);
	}
}

impl Counter {
	/// Create a new counter metric with specified `labels`.
	pub fn new(name: &'static str, description: &'static str) -> Self {
		// Send a register metric operation to node side.
		let metric_update = RuntimeMetricUpdate {
			metric_name: sp_std::vec::Vec::from(name),
			op: RuntimeMetricOp::Register(RuntimeMetricRegisterParams::new(
				sp_std::vec::Vec::from(description),
				None,
			)),
		}
		.encode();

		emit_metric(&metric_update);

		Counter { name }
	}

	/// Increment by `value`.
	pub fn inc_by(&mut self, value: u64) {
		let metric_update = RuntimeMetricUpdate {
			metric_name: sp_std::vec::Vec::from(self.name),
			op: RuntimeMetricOp::IncrementCounter(value),
		}
		.encode();

		emit_metric(&metric_update);
	}

	/// Increment by 1.
	pub fn inc(&mut self) {
		self.inc_by(1);
	}
}
