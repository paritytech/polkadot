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

//! Runtime metric primitives.

use parity_scale_codec::{Decode, Encode};
use sp_std::prelude::*;

/// Metric registration parameters.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricRegisterParams {
	/// Metric description.
	description: Vec<u8>,
	/// Only for counter vec.
	pub labels: Option<RuntimeMetricLabels>,
}

/// Runtime metric operations.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum RuntimeMetricOp {
	/// Register a new metric.
	Register(RuntimeMetricRegisterParams),
	/// Increment a counter metric with labels by value.
	IncrementCounterVec(u64, RuntimeMetricLabelValues),
	/// Increment a counter metric by value.
	IncrementCounter(u64),
}

impl RuntimeMetricRegisterParams {
	/// Create new metric registration params.
	pub fn new(description: Vec<u8>, labels: Option<RuntimeMetricLabels>) -> Self {
		Self { description, labels }
	}
}

/// Runtime metric update event.
#[derive(Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricUpdate {
	/// The name of the metric.
	pub metric_name: Vec<u8>,
	/// The operation applied to the metric.
	pub op: RuntimeMetricOp,
}

fn vec_to_str<'a>(v: &'a Vec<u8>, default: &'static str) -> &'a str {
	return sp_std::str::from_utf8(v).unwrap_or(default)
}

impl RuntimeMetricRegisterParams {
	/// Returns the metric description.
	pub fn description(&self) -> &str {
		vec_to_str(&self.description, "No description provided.")
	}

	/// Returns a label names as an `Option` of `Vec<&str>`.
	pub fn labels(&self) -> Option<Vec<&str>> {
		self.labels.as_ref().map(|labels| labels.as_str_vec())
	}
}

impl RuntimeMetricLabels {
	/// Returns a labels as `Vec<&str>`.
	pub fn as_str_vec(&self) -> Vec<&str> {
		self.0
			.iter()
			.map(|label_vec| vec_to_str(&label_vec.0, "invalid_label"))
			.collect()
	}

	/// Return the inner values as vec.
	pub fn clear(&mut self) {
		self.0.clear();
	}
}

impl From<&[&'static str]> for RuntimeMetricLabels {
	fn from(v: &[&'static str]) -> RuntimeMetricLabels {
		RuntimeMetricLabels(
			v.iter().map(|label| RuntimeMetricLabel(label.as_bytes().to_vec())).collect(),
		)
	}
}

impl RuntimeMetricUpdate {
	/// Returns the metric name.
	pub fn metric_name(&self) -> &str {
		vec_to_str(&self.metric_name, "invalid_metric_name")
	}
}

/// A set of metric labels.
#[derive(Clone, Default, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricLabels(Vec<RuntimeMetricLabel>);

/// A metric label.
#[derive(Clone, Default, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct RuntimeMetricLabel(Vec<u8>);

/// A metric label value.
pub type RuntimeMetricLabelValue = RuntimeMetricLabel;

/// A set of metric label values.
pub type RuntimeMetricLabelValues = RuntimeMetricLabels;

/// Trait for converting Vec<u8> to `&str`.
pub trait AsStr {
	/// Return a str reference.
	fn as_str(&self) -> Option<&str>;
}

impl AsStr for RuntimeMetricLabel {
	fn as_str(&self) -> Option<&str> {
		sp_std::str::from_utf8(&self.0).ok()
	}
}

impl From<&'static str> for RuntimeMetricLabel {
	fn from(s: &'static str) -> Self {
		Self(s.as_bytes().to_vec())
	}
}
